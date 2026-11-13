//! The logic editor: the graph of a mission, checkpoint, stream or SimGroup
//! (objects as nodes, messages and links as wires) on the node-graph widget.
//!
//! The view never writes: edits come back as [`LogicAction`]s that the app
//! records in the workspace (one undoable operation each), after which the
//! view reads the graph again ([`LogicEditor::refresh`]). Node positions and
//! comments live in a sidecar file in the workspace directory
//! (`logic_layout/<scope>.json`), not in the game data. See
//! `docs/logic-graph.md`.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, channel};

use doctor_toolkit::game::{GameInstance, ResourceIndex};
use doctor_toolkit::level::Location;
use doctor_toolkit::logic_graph::edit::{self, Change, NodeTemplate};
use doctor_toolkit::logic_graph::{
    self, BuildOptions, Category, Confidence, LogicGraph, LogicSchema, MessageIndex, MessageState,
    Mission, NodeSource, PinKind, PinRef, ScopeSpec, WireKind, scope,
};
use doctor_toolkit::resource::simgroup::{Kind, Value};
use doctor_toolkit::resource::simgroup_edit::parse_value;
use doctor_toolkit::workspace::Workspace;
use doctor_toolkit_mods::i18n::{tr, trf, trn};
use egui::{Color32, Pos2, RichText};

mod run_mode;

use crate::node_graph::{
    self as ng, Badge, ContextTarget, Graph, GraphEvent, GraphView, LayoutStore, NodeId, Pin,
    PinAddr, Wire, WireStyle,
};

/// Pin type of messages in the widget.
const TY_MESSAGE: u32 = 0;
/// Pin type of GUID links.
const TY_LINK: u32 = 1;
/// Offset of the keys of delay texts in `LogicEditor::texts`.
const DELAY_KEY: usize = 1 << 20;

/// What the user asked for.
pub enum LogicAction {
    /// Change attributes (connect, disconnect, edit a value).
    Apply { changes: Vec<Change>, label: String },
    /// Add nodes: SimGroup, template.
    AddNodes(Vec<(Location, NodeTemplate)>),
    /// Remove objects (by SimGroup and index).
    RemoveNodes(Vec<(Location, usize)>),
    /// Show an object in the 3D level view.
    ShowInLevel { group: Location, object: usize },
    /// Open a SimGroup in its own tab.
    OpenGroup(Location),
}

fn category_color(c: Category) -> Color32 {
    match c {
        Category::Trigger => Color32::from_rgb(190, 115, 40),
        Category::Logic => Color32::from_rgb(55, 95, 165),
        Category::Ui => Color32::from_rgb(125, 75, 155),
        Category::Audio => Color32::from_rgb(35, 130, 125),
        Category::Spawn => Color32::from_rgb(55, 130, 65),
        Category::Job => Color32::from_rgb(160, 130, 35),
        Category::Cinematic => Color32::from_rgb(155, 55, 60),
        Category::Relay => Color32::from_rgb(85, 95, 120),
        Category::Action => Color32::from_rgb(85, 85, 85),
        Category::Object => Color32::from_rgb(105, 80, 55),
        Category::Other => Color32::from_rgb(55, 55, 60),
    }
}

const MESSAGE_COLOR: Color32 = Color32::from_rgb(140, 190, 255);
const ENGINE_COLOR: Color32 = Color32::from_rgb(235, 200, 90);
const LINK_COLOR: Color32 = Color32::from_rgb(170, 170, 170);
const DANGLING_COLOR: Color32 = Color32::from_rgb(235, 90, 80);

/// Which parts of the graph are shown.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Filters {
    engine: bool,
    links: bool,
    hubs: bool,
    empty_pins: bool,
}

impl Default for Filters {
    fn default() -> Self {
        Self {
            engine: true,
            links: true,
            hubs: true,
            empty_pins: true,
        }
    }
}

/// Where a widget pin comes from.
#[derive(Debug, Clone, Copy, PartialEq)]
enum PinSource {
    Graph(usize),
    /// The "linked by" input of a link target.
    LinkedBy,
}

/// The view of one logic graph tab.
pub struct LogicEditor {
    game: Option<Arc<GameInstance>>,
    index: Option<Arc<ResourceIndex>>,
    missions: Vec<Mission>,
    scope_text: String,
    spec: Option<ScopeSpec>,
    graph: Option<LogicGraph>,
    errors: Vec<String>,
    shown: Graph,
    /// Widget pins → graph pins, per node (inputs, outputs).
    pins: HashMap<NodeId, (Vec<PinSource>, Vec<PinSource>)>,
    /// Widget wires → graph wires.
    wire_of: Vec<Option<usize>>,
    view: GraphView,
    filters: Filters,
    stale: bool,
    selected: Option<usize>,
    /// Text being edited in the side panel: (node, pin) → text.
    texts: HashMap<(usize, usize), String>,
    search: String,
    status: String,
    store: LayoutStore,
    layout_path: Option<PathBuf>,
    /// Where new nodes go, in the order they were asked for.
    pending_place: Vec<Pos2>,
    /// Objects to select once the graph is loaded.
    pending_select: Option<(Location, usize)>,
    clipboard: Vec<(Location, usize)>,
    messages: Option<Arc<MessageIndex>>,
    messages_job: Option<Receiver<MessageIndex>>,
    workspace_dir: Option<PathBuf>,
    /// Text to find once the graph is loaded (the environment preset).
    find_after_load: Option<String>,
    /// Show the found node in the level once loaded (the environment preset).
    pub preset_show: bool,
    /// «Run» mode: the logic runs without the game (stage 38).
    runm: run_mode::RunMode,
}

impl LogicEditor {
    pub fn new(scope: &str) -> Self {
        Self {
            game: None,
            index: None,
            missions: Vec::new(),
            scope_text: scope.to_owned(),
            spec: None,
            graph: None,
            errors: Vec::new(),
            shown: Graph::default(),
            pins: HashMap::new(),
            wire_of: Vec::new(),
            view: GraphView::default(),
            filters: Filters::default(),
            stale: !scope.is_empty(),
            selected: None,
            texts: HashMap::new(),
            search: String::new(),
            status: String::new(),
            store: LayoutStore::default(),
            layout_path: None,
            pending_place: Vec::new(),
            pending_select: None,
            clipboard: Vec::new(),
            messages: None,
            messages_job: None,
            workspace_dir: None,
            find_after_load: None,
            preset_show: false,
            runm: run_mode::RunMode::default(),
        }
    }

    /// The scope text, kept in the dock layout.
    pub fn scope(&self) -> &str {
        &self.scope_text
    }

    /// A short title for the tab.
    pub fn title(&self) -> String {
        match (&self.spec, &self.game) {
            (Some(spec), Some(game)) => {
                let label = spec.label(game);
                let short = label.rsplit('/').next().unwrap_or(&label).to_owned();
                format!("{}: {short}", tr("logic.tab"))
            }
            _ => tr("logic.tab").to_owned(),
        }
    }

    pub fn open_game(&mut self, game: Arc<GameInstance>, index: Arc<ResourceIndex>) {
        self.missions = scope::missions(&game);
        self.workspace_dir = Some(Workspace::default_dir(game.root()));
        self.game = Some(game);
        self.index = Some(index);
        self.messages = None;
        self.messages_job = None;
        self.stale = !self.scope_text.is_empty();
    }

    /// Selects the nodes matching `text` once the graph is loaded.
    pub fn find_after_load(&mut self, text: &str) {
        self.find_after_load = Some(text.to_owned());
    }

    /// Shows another scope, selecting `select` when it is loaded.
    pub fn set_scope(&mut self, scope: &str, select: Option<(Location, usize)>) {
        if self.scope_text != scope || self.graph.is_none() {
            self.scope_text = scope.to_owned();
            self.stale = true;
        } else if let Some(sel) = select {
            self.select_object(sel);
        }
        self.pending_select = select;
    }

    /// The workspace changed: read the graph again.
    pub fn refresh(&mut self) {
        if self.graph.is_some() {
            self.stale = true;
            self.messages = None;
            self.messages_job = None;
        }
    }

    fn node_key(&self, graph: &LogicGraph, n: usize) -> Option<String> {
        let node = graph.nodes.get(n)?;
        Some(match &node.source {
            NodeSource::Object { group, .. } => {
                format!(
                    "{}#{}/{}",
                    node.stream,
                    group.resource,
                    node.guid.as_deref()?
                )
            }
            NodeSource::Engine => "engine".into(),
            NodeSource::Hub { message } => format!("hub/{message:08x}"),
        })
    }

    fn load(&mut self, ws: Option<&Workspace>) {
        self.stale = false;
        let (Some(game), Some(index)) = (self.game.clone(), self.index.clone()) else {
            return;
        };
        let spec = match ScopeSpec::parse(&game, self.scope_text.trim()) {
            Ok(spec) => spec,
            Err(e) => {
                self.status = e.to_string();
                return;
            }
        };
        let options = BuildOptions {
            hub_threshold: if self.filters.hubs { 48 } else { 0 },
            engine_node: self.filters.engine,
            keep_unwired: false,
        };
        match scope::build(&game, &index, ws, &spec, options) {
            Ok((mut graph, errors)) => {
                if let Some(m) = &self.messages {
                    graph.annotate(m);
                }
                self.errors = errors
                    .iter()
                    .map(|(l, e)| {
                        format!(
                            "{} #{}: {e}",
                            game.streams()[l.stream].stream_name(),
                            l.resource
                        )
                    })
                    .collect();
                let o = graph.overview();
                self.status = trf(
                    "logic.loaded",
                    &[
                        ("nodes", &trn("count.nodes", o.nodes as u64, &[])),
                        ("wires", &trn("count.wires", o.wires as u64, &[])),
                        ("messages", &trn("count.messages", o.messages as u64, &[])),
                    ],
                );
                let new_scope = self.spec.as_ref() != Some(&spec);
                let dir = ws
                    .map(|w| w.dir().to_path_buf())
                    .or(self.workspace_dir.clone());
                self.layout_path = dir.map(|d| {
                    d.join("logic_layout")
                        .join(format!("{}.json", file_key(&self.scope_text)))
                });
                if new_scope {
                    self.store = self
                        .layout_path
                        .as_deref()
                        .map(LayoutStore::load)
                        .unwrap_or_default();
                    self.selected = None;
                    self.view.set_selection([]);
                } else {
                    self.capture_layout();
                }
                // Keep the selected object across reloads.
                let selected = self
                    .selected
                    .and_then(|n| self.graph.as_ref()?.nodes.get(n)?.object());
                self.spec = Some(spec);
                self.graph = Some(graph);
                self.run_reset();
                self.rebuild_shown();
                self.place_nodes(new_scope);
                if let Some(sel) = self.pending_select.take().or(selected) {
                    self.select_object(sel);
                    if new_scope {
                        self.view.frame_selection();
                    }
                }
                if self.messages.is_none() && self.messages_job.is_none() {
                    self.start_messages();
                }
                if let Some(text) = self.find_after_load.take() {
                    self.search = text;
                    self.select_matches();
                    self.rebuild_shown_keep_positions();
                }
            }
            Err(e) => self.status = e.to_string(),
        }
    }

    fn start_messages(&mut self) {
        let (Some(game), Some(index)) = (self.game.clone(), self.index.clone()) else {
            return;
        };
        let ws_dir = self.workspace_dir.clone();
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let ws = ws_dir
                .filter(|d| Workspace::exists(d))
                .and_then(|d| Workspace::open(&d).ok());
            let _ = tx.send(MessageIndex::build(&game, &index, ws.as_ref()));
        });
        self.messages_job = Some(rx);
    }

    fn poll_messages(&mut self) {
        if let Some(rx) = &self.messages_job
            && let Ok(index) = rx.try_recv()
        {
            let index = Arc::new(index);
            if let Some(g) = &mut self.graph {
                g.annotate(&index);
            }
            self.messages = Some(index);
            self.messages_job = None;
            self.rebuild_shown_keep_positions();
        }
    }

    fn select_object(&mut self, (group, object): (Location, usize)) {
        if let Some(n) = self.graph.as_ref().and_then(|g| g.node_of(group, object)) {
            self.selected = Some(n);
            self.view.set_selection([NodeId(n as u64)]);
        }
    }

    fn capture_layout(&mut self) {
        let Some(graph) = &self.graph else { return };
        let keys: HashMap<NodeId, String> = (0..graph.nodes.len())
            .filter_map(|n| Some((NodeId(n as u64), self.node_key(graph, n)?)))
            .collect();
        self.store
            .capture(&self.shown, &|id| keys.get(&id).cloned());
    }

    fn save_layout(&mut self) {
        self.capture_layout();
        if let Some(path) = &self.layout_path
            && let Err(e) = self.store.save(path)
        {
            self.status = format!("{}: {e}", tr("logic.layout_not_saved"));
        }
    }

    fn rebuild_shown_keep_positions(&mut self) {
        let positions: HashMap<NodeId, Pos2> =
            self.shown.nodes.iter().map(|n| (n.id, n.pos)).collect();
        let comments = std::mem::take(&mut self.shown.comments);
        self.rebuild_shown();
        for n in &mut self.shown.nodes {
            if let Some(p) = positions.get(&n.id) {
                n.pos = *p;
            }
        }
        self.shown.comments = comments;
    }

    /// Positions from the sidecar; the rest by the layered layout.
    fn place_nodes(&mut self, new_scope: bool) {
        let Some(graph) = &self.graph else { return };
        let keys: HashMap<NodeId, String> = (0..graph.nodes.len())
            .filter_map(|n| Some((NodeId(n as u64), self.node_key(graph, n)?)))
            .collect();
        let missing = self
            .store
            .restore(&mut self.shown, &|id| keys.get(&id).cloned());
        if missing.is_empty() {
            return;
        }
        if missing.len() == self.shown.nodes.len() {
            let pos = ng::layered(&self.shown, None, &ng::LayoutOptions::default());
            for n in &mut self.shown.nodes {
                n.pos = pos[&n.id];
            }
            if new_scope {
                self.view.frame_all();
            }
            return;
        }
        // New nodes: where the user asked, else laid out below the rest.
        let bottom = self
            .shown
            .nodes
            .iter()
            .filter(|n| !missing.contains(&n.id))
            .map(|n| n.rect().max.y)
            .fold(0.0, f32::max);
        let only: HashSet<NodeId> = missing.iter().copied().collect();
        let pos = ng::layered(&self.shown, Some(&only), &ng::LayoutOptions::default());
        for id in missing {
            let p = if self.pending_place.is_empty() {
                pos[&id] + egui::vec2(0.0, bottom + 80.0)
            } else {
                self.pending_place.remove(0)
            };
            if let Some(n) = self.shown.node_mut(id) {
                n.pos = p;
            }
        }
        self.pending_place.clear();
        self.save_layout();
    }

    /// Makes the widget graph from the logic graph and the filters.
    fn rebuild_shown(&mut self) {
        let Some(graph) = &self.graph else { return };
        let schema = LogicSchema::gf2();
        let mut shown = Graph::default();
        self.pins.clear();
        self.wire_of.clear();
        let hidden = |n: usize| match graph.nodes[n].source {
            NodeSource::Engine => !self.filters.engine,
            _ => false,
        };
        let link_targets: HashSet<usize> = graph
            .wires
            .iter()
            .filter(|w| w.kind == WireKind::Link)
            .map(|w| w.to.node)
            .collect();
        let search = self.search.to_ascii_lowercase();
        for (i, node) in graph.nodes.iter().enumerate() {
            if hidden(i) {
                continue;
            }
            let id = NodeId(i as u64);
            let mut n = ng::Node::new(id, node.title());
            n.header_color = category_color(node.category);
            n.subtitle = match node.source {
                NodeSource::Object { group, object } => {
                    let stream = node.stream.rsplit('/').next().unwrap_or(&node.stream);
                    format!(
                        "{} · {stream} #{} · {object}",
                        node.class_name, group.resource
                    )
                }
                NodeSource::Engine => tr("logic.engine_node").to_owned(),
                NodeSource::Hub { .. } => tr("logic.hub_node").to_owned(),
            };
            if node.name.is_empty() && matches!(node.source, NodeSource::Object { .. }) {
                n.title = node.class_name.clone();
            }
            let (mut ins, mut outs) = (Vec::new(), Vec::new());
            // Lists (send_1 … send_8): set ones and the first free one.
            let mut list_free_shown: HashSet<String> = HashSet::new();
            for (p, pin) in node.pins.iter().enumerate() {
                let (ty, kind_ok) = match pin.kind {
                    PinKind::In | PinKind::Out => (TY_MESSAGE, true),
                    PinKind::Link => (TY_LINK, self.filters.links),
                    _ => (TY_MESSAGE, false),
                };
                if !kind_ok {
                    continue;
                }
                let set = pin.message.is_some() || pin.target.is_some();
                if !set {
                    let base = pin
                        .name
                        .trim_end_matches(|c: char| c.is_ascii_digit())
                        .to_owned();
                    let is_list = base.ends_with('_') && base.len() < pin.name.len();
                    if is_list {
                        if !list_free_shown.insert(format!("{:?}{base}", pin.kind)) {
                            continue;
                        }
                    } else if !self.filters.empty_pins || pin.kind == PinKind::Link {
                        continue;
                    }
                }
                let mut label = match (&pin.message, pin.delay) {
                    (Some(m), Some(d)) if d != 0.0 => format!("{}: {m} (+{d}s)", pin.name),
                    (Some(m), _) => format!("{}: {m}", pin.name),
                    (None, _) if pin.kind == PinKind::Link => {
                        trf("logic.pin_link_label", &[("pin", &pin.name)])
                    }
                    (None, _) => pin.name.clone(),
                };
                if !search.is_empty()
                    && pin
                        .message
                        .as_deref()
                        .is_some_and(|m| m.to_ascii_lowercase().contains(&search))
                {
                    label = format!("» {label}");
                }
                let state = pin.hash.and_then(|h| graph.messages.get(&h));
                let color = match (pin.kind, state) {
                    (PinKind::Link, _) => LINK_COLOR,
                    (_, Some(m)) if m.dangling_in_game() => DANGLING_COLOR,
                    (_, Some(m)) if m.engine => ENGINE_COLOR,
                    _ => MESSAGE_COLOR,
                };
                let conf = match pin.conf {
                    Some(Confidence::Exe) => tr("logic.conf_exe"),
                    Some(Confidence::Data) => tr("logic.conf_data"),
                    _ => tr("logic.conf_guess"),
                };
                let mut tip = format!("{} ({conf})", pin.name);
                if let Some(m) = state {
                    tip += &format!("\n{}: {}", m.name, state_text(m.state));
                    if let Some((s, l)) = m.elsewhere {
                        tip += &format!(
                            "\n{}",
                            trf(
                                "logic.elsewhere",
                                &[
                                    ("senders", &trn("count.senders", s as u64, &[])),
                                    ("listeners", &trn("count.listeners", l as u64, &[])),
                                ],
                            )
                        );
                    }
                }
                if let Some(t) = &pin.target {
                    tip += &format!("\n{t}");
                }
                let wp = Pin::new(label, ty).color(color).set(set).tooltip(tip);
                if pin.kind == PinKind::In {
                    n.inputs.push(wp);
                    ins.push(PinSource::Graph(p));
                } else {
                    n.outputs.push(wp);
                    outs.push(PinSource::Graph(p));
                }
            }
            if link_targets.contains(&i) && self.filters.links {
                n.inputs.push(
                    Pin::new(tr("logic.linked_by"), TY_LINK)
                        .color(LINK_COLOR)
                        .set(true),
                );
                ins.push(PinSource::LinkedBy);
            }
            // Key parameters.
            for pin in node.pins.iter().filter(|p| p.kind == PinKind::Param) {
                if let Some(v) = &pin.value
                    && schema
                        .attr(pin.class, pin.index)
                        .is_some_and(|a| a.name.is_some())
                    && n.lines.len() < 3
                {
                    n.lines.push(format!("{} = {v}", pin.name));
                }
            }
            let dangling = node
                .pins
                .iter()
                .filter_map(|p| p.hash)
                .filter(|h| graph.messages.get(h).is_some_and(|m| m.dangling_in_game()));
            let count = dangling.count();
            if count > 0 {
                n.badges.push(Badge {
                    text: "!".into(),
                    color: DANGLING_COLOR,
                    tooltip: trn("logic.badge_dangling_n", count as u64, &[]),
                });
            }
            self.pins.insert(id, (ins, outs));
            shown.nodes.push(n);
        }
        // Wires.
        let find =
            |pins: &HashMap<NodeId, (Vec<PinSource>, Vec<PinSource>)>, r: PinRef, output: bool| {
                let (ins, outs) = pins.get(&NodeId(r.node as u64))?;
                let list = if output { outs } else { ins };
                list.iter().position(|s| *s == PinSource::Graph(r.pin))
            };
        for (wi, w) in graph.wires.iter().enumerate() {
            if hidden(w.from.node) || hidden(w.to.node) {
                continue;
            }
            let (from_pin, to_pin) = match w.kind {
                WireKind::Link => {
                    if !self.filters.links {
                        continue;
                    }
                    let to = self
                        .pins
                        .get(&NodeId(w.to.node as u64))
                        .and_then(|(ins, _)| ins.iter().position(|s| *s == PinSource::LinkedBy));
                    (find(&self.pins, w.from, true), to)
                }
                _ => (
                    find(&self.pins, w.from, true),
                    find(&self.pins, w.to, false),
                ),
            };
            let (Some(a), Some(b)) = (from_pin, to_pin) else {
                continue;
            };
            let mut wire = Wire::new(
                (NodeId(w.from.node as u64), a),
                (NodeId(w.to.node as u64), b),
            );
            let name = w
                .message
                .and_then(|h| graph.messages.get(&h))
                .map(|m| m.name.clone());
            match w.kind {
                WireKind::Message => wire.color = MESSAGE_COLOR,
                WireKind::Engine => {
                    wire.color = ENGINE_COLOR;
                    wire.style = WireStyle::Dashed;
                }
                WireKind::Link => {
                    wire.color = LINK_COLOR;
                    wire.style = WireStyle::Dotted;
                }
            }
            wire.label = name;
            shown.wires.push(wire);
            self.wire_of.push(Some(wi));
        }
        shown.comments = std::mem::take(&mut self.shown.comments);
        self.run_highlight(&mut shown);
        self.shown = shown;
    }

    /// The tab: toolbar, side panel, canvas. Returns an edit to record.
    pub fn ui(&mut self, ui: &mut egui::Ui, ws: Option<&Workspace>) -> Option<LogicAction> {
        if self.stale {
            self.load(ws);
        }
        self.poll_messages();
        if self.messages_job.is_some() {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(100));
        }
        let mut action = None;
        if self.preset_show
            && self.find_after_load.is_none()
            && let Some((group, object)) = self.graph.as_ref().and_then(|g| {
                self.view
                    .selection()
                    .iter()
                    .filter_map(|id| g.nodes.get(id.0 as usize))
                    .find(|n| n.position.is_some())?
                    .object()
            })
        {
            self.preset_show = false;
            action = Some(LogicAction::ShowInLevel { group, object });
        }
        self.toolbar(ui, ws);
        if self.graph.is_none() {
            ui.add_space(20.0);
            ui.vertical_centered(|ui| {
                ui.label(tr("logic.empty_hint"));
            });
            return None;
        }
        if self.runm.on {
            egui::Panel::bottom("logic-run")
                .resizable(true)
                .default_size(280.0)
                .show(ui, |ui| self.run_panel(ui, ws));
        }
        egui::Panel::right("logic-side")
            .resizable(true)
            .default_size(320.0)
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .auto_shrink(false)
                    .show(ui, |ui| {
                        action = self.side_panel(ui).or(action.take());
                    });
            });
        let mut menu_action = None;
        let mut menu_cmd: Option<MenuCmd> = None;
        let graph_ref = self.graph.clone();
        let wire_of = self.wire_of.clone();
        let pins = self.pins.clone();
        let clipboard_empty = self.clipboard.is_empty();
        let response = self.view.show(ui, &mut self.shown, &mut |ui, target| {
            if let Some(graph) = &graph_ref {
                menu_cmd = context_menu(ui, graph, &wire_of, &pins, target, clipboard_empty)
                    .or(menu_cmd.take());
            }
        });
        if let Some(cmd) = menu_cmd {
            menu_action = self.run_menu(cmd);
        }
        for event in response.events {
            if let Some(a) = self.on_event(event) {
                action = Some(a);
            }
        }
        action.or(menu_action)
    }

    fn toolbar(&mut self, ui: &mut egui::Ui, _ws: Option<&Workspace>) {
        ui.horizontal_wrapped(|ui| {
            ui.label(tr("logic.scope"));
            let edit = ui.add(
                egui::TextEdit::singleline(&mut self.scope_text)
                    .desired_width(320.0)
                    .hint_text("mission:… checkpoint:… stream:… group:…#…"),
            );
            if edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                self.stale = true;
            }
            egui::ComboBox::from_id_salt(ui.scope_id().with("missions"))
                .selected_text(tr("logic.missions"))
                .height(400.0)
                .show_ui(ui, |ui| {
                    for m in &self.missions {
                        if ui.selectable_label(false, &m.name).clicked() {
                            self.scope_text = format!("mission:{}", m.name);
                            self.stale = true;
                        }
                    }
                });
            if ui.button(tr("logic.load")).clicked() {
                self.stale = true;
            }
            ui.separator();
            if ui
                .button(tr("logic.auto_layout"))
                .on_hover_text(tr("logic.auto_layout_hint"))
                .clicked()
            {
                self.auto_layout();
            }
            if ui
                .button(tr("logic.frame"))
                .on_hover_text(tr("logic.frame_hint"))
                .clicked()
            {
                self.view.frame_all();
            }
            if ui
                .toggle_value(&mut self.runm.on, tr("logic_run.mode"))
                .on_hover_text(tr("logic_run.mode_hint"))
                .changed()
            {
                self.rebuild_shown_keep_positions();
            }
            ui.checkbox(&mut self.view.show_minimap, tr("logic.minimap"));
            let before = self.filters;
            ui.checkbox(&mut self.filters.engine, tr("logic.show_engine"));
            ui.checkbox(&mut self.filters.links, tr("logic.show_links"));
            ui.checkbox(&mut self.filters.hubs, tr("logic.show_hubs"))
                .on_hover_text(tr("logic.show_hubs_hint"));
            ui.checkbox(&mut self.filters.empty_pins, tr("logic.empty_pins"));
            if before != self.filters {
                if before.hubs != self.filters.hubs || before.engine != self.filters.engine {
                    self.capture_layout();
                    self.stale = true;
                } else {
                    self.rebuild_shown_keep_positions();
                }
            }
            ui.separator();
            let search = ui.add(
                egui::TextEdit::singleline(&mut self.search)
                    .desired_width(160.0)
                    .hint_text(tr("logic.find_message")),
            );
            if search.changed() {
                self.rebuild_shown_keep_positions();
            }
            if search.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                self.select_matches();
            }
        });
        ui.horizontal(|ui| {
            ui.label(RichText::new(&self.status).small());
            if self.messages_job.is_some() {
                ui.spinner();
                ui.label(RichText::new(tr("logic.indexing")).small());
            }
            if !self.errors.is_empty() {
                ui.colored_label(
                    Color32::YELLOW,
                    trn("logic.unreadable_groups", self.errors.len() as u64, &[]),
                )
                .on_hover_text(self.errors.join("\n"));
            }
        });
    }

    fn select_matches(&mut self) {
        let Some(graph) = &self.graph else { return };
        let want = self.search.to_ascii_lowercase();
        if want.is_empty() {
            return;
        }
        let ids: Vec<NodeId> = graph
            .nodes
            .iter()
            .enumerate()
            .filter(|(_, n)| {
                n.title().to_ascii_lowercase().contains(&want)
                    || n.class_name.to_ascii_lowercase() == want
                    || n.stream.to_ascii_lowercase().ends_with(&want)
                    || n.pins.iter().any(|p| {
                        p.message
                            .as_deref()
                            .is_some_and(|m| m.to_ascii_lowercase().contains(&want))
                    })
            })
            .map(|(i, _)| NodeId(i as u64))
            .collect();
        self.status = trn("logic.found_nodes", ids.len() as u64, &[]);
        self.selected = ids.first().map(|id| id.0 as usize);
        self.view.set_selection(ids);
        self.view.frame_selection();
    }

    /// Lays out the selection (two or more nodes) where it is, or everything.
    fn auto_layout(&mut self) {
        let selection: HashSet<NodeId> = self.view.selection().iter().copied().collect();
        let only = (selection.len() > 1).then_some(&selection);
        let pos = ng::layered(&self.shown, only, &ng::LayoutOptions::default());
        let offset = match only {
            Some(sel) => {
                let corner = self
                    .shown
                    .nodes
                    .iter()
                    .filter(|n| sel.contains(&n.id))
                    .map(|n| n.pos)
                    .reduce(|a, b| a.min(b))
                    .unwrap_or(Pos2::ZERO);
                corner.to_vec2()
            }
            None => {
                self.shown.comments.clear();
                egui::Vec2::ZERO
            }
        };
        for n in &mut self.shown.nodes {
            if let Some(p) = pos.get(&n.id) {
                n.pos = *p + offset;
            }
        }
        if only.is_some() {
            self.view.frame_selection();
        } else {
            self.view.frame_all();
        }
        self.save_layout();
    }

    fn on_event(&mut self, event: GraphEvent) -> Option<LogicAction> {
        let graph = self.graph.as_ref()?;
        match event {
            GraphEvent::Selection(ids) => {
                self.selected = ids.first().map(|id| id.0 as usize);
                None
            }
            GraphEvent::Moved(_) | GraphEvent::CommentsChanged => {
                self.save_layout();
                None
            }
            GraphEvent::Activate(id) => {
                let (group, object) = graph.nodes.get(id.0 as usize)?.object()?;
                if graph.nodes[id.0 as usize].position.is_some() {
                    Some(LogicAction::ShowInLevel { group, object })
                } else {
                    Some(LogicAction::OpenGroup(group))
                }
            }
            GraphEvent::Connect { from, to } => self.connect(from, to),
            GraphEvent::Disconnect { wire } => self.disconnect(wire),
            GraphEvent::Delete(ids) => self.remove(&ids),
            GraphEvent::Copy(ids) => {
                self.clipboard = ids
                    .iter()
                    .filter_map(|id| graph.nodes.get(id.0 as usize)?.object())
                    .collect();
                self.status = trf(
                    "logic.copied",
                    &[(
                        "nodes",
                        &trn("count.nodes", self.clipboard.len() as u64, &[]),
                    )],
                );
                None
            }
            GraphEvent::Paste(at) => self.paste(at),
        }
    }

    fn graph_pin(&self, addr: PinAddr) -> Option<PinRef> {
        let (ins, outs) = self.pins.get(&addr.node)?;
        let list = if addr.output { outs } else { ins };
        match list.get(addr.pin)? {
            PinSource::Graph(p) => Some(PinRef {
                node: addr.node.0 as usize,
                pin: *p,
            }),
            PinSource::LinkedBy => None,
        }
    }

    fn connect(&mut self, from: PinAddr, to: PinAddr) -> Option<LogicAction> {
        let graph = self.graph.as_ref()?;
        let (a, b) = (self.graph_pin(from)?, self.graph_pin(to)?);
        let fresh = || {
            let n = &graph.nodes[a.node];
            let base = if n.name.is_empty() {
                &n.class_name
            } else {
                &n.name
            };
            edit::fresh_message(
                graph,
                &format!("{base}_{}", graph.nodes[a.node].pins[a.pin].name),
            )
        };
        match edit::plan_connect(graph, a, b, fresh) {
            Ok(changes) if changes.is_empty() => None,
            Ok(changes) => Some(LogicAction::Apply {
                label: tr("logic.op_connect").to_owned(),
                changes,
            }),
            Err(e) => {
                self.status = e.to_string();
                None
            }
        }
    }

    fn disconnect(&mut self, wire: usize) -> Option<LogicAction> {
        let graph = self.graph.as_ref()?;
        let w = (*self.wire_of.get(wire)?)?;
        match edit::plan_disconnect(graph, w) {
            Ok(changes) => Some(LogicAction::Apply {
                label: tr("logic.op_disconnect").to_owned(),
                changes,
            }),
            Err(e) => {
                self.status = e.to_string();
                None
            }
        }
    }

    fn remove(&mut self, ids: &[NodeId]) -> Option<LogicAction> {
        let graph = self.graph.as_ref()?;
        let nodes: Vec<(Location, usize)> = ids
            .iter()
            .filter_map(|id| graph.nodes.get(id.0 as usize)?.object())
            .collect();
        (!nodes.is_empty()).then_some(LogicAction::RemoveNodes(nodes))
    }

    fn paste(&mut self, at: Pos2) -> Option<LogicAction> {
        if self.clipboard.is_empty() {
            return None;
        }
        let graph = self.graph.as_ref()?;
        let first = graph.node_of(self.clipboard[0].0, self.clipboard[0].1);
        let origin = first
            .and_then(|n| self.shown.node(NodeId(n as u64)))
            .map_or(at, |n| n.pos);
        let mut adds = Vec::new();
        for &(group, object) in &self.clipboard {
            let pos = graph
                .node_of(group, object)
                .and_then(|n| self.shown.node(NodeId(n as u64)))
                .map_or(at, |n| at + (n.pos - origin));
            self.pending_place.push(pos);
            adds.push((group, NodeTemplate::Copy(group, object)));
        }
        Some(LogicAction::AddNodes(adds))
    }

    /// A template for a new node of `class`: an object of the class in the
    /// graph, else anywhere in the game, else empty packets.
    fn template_for(&self, class: u32) -> NodeTemplate {
        if let Some((g, o)) = self.graph.as_ref().and_then(|g| {
            g.nodes
                .iter()
                .find(|n| n.class == class)
                .and_then(|n| n.object())
        }) {
            return NodeTemplate::Copy(g, o);
        }
        if let Some(index) = &self.messages
            && let Some(site) = index
                .by_hash
                .values()
                .flat_map(|m| m.senders.iter().chain(&m.listeners))
                .find(|s| s.class == class)
        {
            return NodeTemplate::Copy(site.group, site.object);
        }
        NodeTemplate::Empty(class)
    }

    /// The SimGroup new nodes go into: the selected node's, else the first.
    fn target_group(&self) -> Option<Location> {
        let graph = self.graph.as_ref()?;
        self.selected
            .and_then(|n| graph.nodes.get(n)?.object())
            .map(|(g, _)| g)
            .or_else(|| graph.groups.first().map(|(l, _)| *l))
    }

    fn run_menu(&mut self, cmd: MenuCmd) -> Option<LogicAction> {
        match cmd {
            MenuCmd::Disconnect(w) => {
                let graph = self.graph.as_ref()?;
                match edit::plan_disconnect(graph, w) {
                    Ok(changes) => Some(LogicAction::Apply {
                        label: tr("logic.op_disconnect").to_owned(),
                        changes,
                    }),
                    Err(e) => {
                        self.status = e.to_string();
                        None
                    }
                }
            }
            MenuCmd::Clear(p) => {
                let graph = self.graph.as_ref()?;
                let at = edit::attr_of(graph, p).ok()?;
                Some(LogicAction::Apply {
                    label: tr("logic.op_clear").to_owned(),
                    changes: vec![Change { at, value: None }],
                })
            }
            MenuCmd::Remove(n) => self.remove(&[NodeId(n as u64)]),
            MenuCmd::ShowInLevel(n) => {
                let (group, object) = self.graph.as_ref()?.nodes.get(n)?.object()?;
                Some(LogicAction::ShowInLevel { group, object })
            }
            MenuCmd::OpenGroup(n) => {
                let (group, _) = self.graph.as_ref()?.nodes.get(n)?.object()?;
                Some(LogicAction::OpenGroup(group))
            }
            MenuCmd::Add(class, at) => {
                let group = self.target_group()?;
                self.pending_place.push(at);
                Some(LogicAction::AddNodes(vec![(
                    group,
                    self.template_for(class),
                )]))
            }
            MenuCmd::Paste(at) => self.paste(at),
            MenuCmd::Comment => {
                let ids: Vec<NodeId> = self.view.selection().iter().copied().collect();
                if let Some(c) = self.shown.comment_around(&ids, tr("logic.comment")) {
                    self.shown.comments.push(c);
                    self.save_layout();
                }
                None
            }
            MenuCmd::Layout => {
                self.auto_layout();
                None
            }
            MenuCmd::Frame => {
                self.view.frame_all();
                None
            }
        }
    }

    fn side_panel(&mut self, ui: &mut egui::Ui) -> Option<LogicAction> {
        let graph = self.graph.clone()?;
        let Some(n) = self.selected.filter(|&n| n < graph.nodes.len()) else {
            return self.overview_panel(ui, &graph);
        };
        let node = &graph.nodes[n];
        let mut action = None;
        ui.heading(node.title());
        ui.label(RichText::new(format!("{} · {}", node.class_name, node.category.name())).weak());
        if let NodeSource::Object { group, object } = node.source {
            ui.label(format!(
                "{} #{} · {} {object}",
                node.stream,
                group.resource,
                tr("logic.object")
            ));
            if let Some(g) = &node.guid {
                ui.label(RichText::new(format!("GUID {g}")).small().monospace());
            }
            ui.horizontal_wrapped(|ui| {
                if node.position.is_some() && ui.button(tr("logic.show_in_level")).clicked() {
                    action = Some(LogicAction::ShowInLevel { group, object });
                }
                if ui.button(tr("logic.open_group")).clicked() {
                    action = Some(LogicAction::OpenGroup(group));
                }
                if ui.button(tr("logic.remove_node")).clicked() {
                    action = Some(LogicAction::RemoveNodes(vec![(group, object)]));
                }
            });
        }
        ui.separator();
        let editable = matches!(node.source, NodeSource::Object { .. });
        // Delays are edited under their `out` pin.
        let delays: HashSet<(u32, u16)> = node
            .pins
            .iter()
            .filter(|p| p.kind == PinKind::Out)
            .filter_map(|p| Some((p.class, LogicSchema::gf2().attr(p.class, p.index)?.delay?)))
            .collect();
        egui::Grid::new(ui.scope_id().with("pins"))
            .num_columns(2)
            .striped(true)
            .show(ui, |ui| {
                for (p, pin) in node.pins.iter().enumerate() {
                    if !matches!(
                        pin.kind,
                        PinKind::In
                            | PinKind::Out
                            | PinKind::Link
                            | PinKind::Asset
                            | PinKind::Param
                    ) {
                        continue;
                    }
                    if pin.kind == PinKind::Param
                        && ((pin.value.is_none() && !editable)
                            || delays.contains(&(pin.class, pin.index)))
                    {
                        continue;
                    }
                    let arrow = match pin.kind {
                        PinKind::In => "> ",
                        PinKind::Out => "< ",
                        PinKind::Link => "@ ",
                        PinKind::Asset => "# ",
                        PinKind::Param => "",
                    };
                    let conf = match pin.conf {
                        Some(Confidence::Exe) => "",
                        Some(Confidence::Data) => " *",
                        _ if pin.kind == PinKind::Param => "",
                        _ => " ?",
                    };
                    ui.label(format!("{arrow}{}{conf}", pin.name))
                        .on_hover_text(pin_help(pin.kind, pin.conf));
                    let current = match pin.kind {
                        PinKind::In | PinKind::Out => pin.message.clone().unwrap_or_default(),
                        PinKind::Link | PinKind::Asset => pin.target.clone().unwrap_or_default(),
                        PinKind::Param => pin.value.clone().unwrap_or_default(),
                    };
                    if !editable || pin.kind == PinKind::Asset || pin.kind == PinKind::Link {
                        ui.label(RichText::new(current).monospace());
                        ui.end_row();
                        continue;
                    }
                    let key = (n, p);
                    let text = self.texts.entry(key).or_insert_with(|| {
                        // Parameters: the editable text of the stored value.
                        current.clone()
                    });
                    let r = ui.add(egui::TextEdit::singleline(text).desired_width(170.0));
                    if r.lost_focus()
                        && ui.input(|i| i.key_pressed(egui::Key::Enter))
                        && *text != current
                    {
                        let text = text.clone();
                        self.texts.remove(&key);
                        action = self.set_value(&graph, n, p, &text);
                    } else if !r.has_focus() && *text != current && !r.lost_focus() {
                        *text = current;
                    }
                    ui.end_row();
                    if pin.kind == PinKind::Out
                        && pin.message.is_some()
                        && let Some(d) = LogicSchema::gf2()
                            .attr(pin.class, pin.index)
                            .and_then(|a| a.delay)
                    {
                        let mut delay = pin.delay.unwrap_or(0.0);
                        ui.label(format!("    {}", tr("logic.delay")));
                        // Committed with Enter, like the other values.
                        let current = format!("{delay}");
                        let key = (n, p + DELAY_KEY);
                        let text = self.texts.entry(key).or_insert_with(|| current.clone());
                        let r = ui.add(egui::TextEdit::singleline(text).desired_width(70.0));
                        if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                            let parsed = text.trim().trim_end_matches('s').trim().parse::<f32>();
                            self.texts.remove(&key);
                            match parsed {
                                Ok(v) if v.is_finite() && v >= 0.0 => {
                                    delay = v;
                                    if let Some((group, object)) = node.object() {
                                        let at = edit::AttrAt {
                                            group,
                                            object,
                                            class: pin.class,
                                            index: d,
                                        };
                                        action = Some(LogicAction::Apply {
                                            label: tr("logic.op_delay").to_owned(),
                                            changes: vec![Change {
                                                at,
                                                value: (delay != 0.0)
                                                    .then_some(Value::Float(delay)),
                                            }],
                                        });
                                    }
                                }
                                _ => self.status = tr("logic.bad_delay").to_owned(),
                            }
                        } else if !r.has_focus() && *text != current {
                            *text = current;
                        }
                        ui.end_row();
                    }
                }
            });
        // Who else sends or hears the messages of this node.
        let mut jump = None;
        if let Some(index) = self.messages.clone() {
            ui.separator();
            ui.label(RichText::new(tr("logic.in_the_game")).strong());
            for pin in node.pins.iter().filter(|p| p.message.is_some()) {
                let Some(sites) = index.who(pin.message.as_deref().unwrap_or_default()) else {
                    continue;
                };
                let header = format!(
                    "{} — {} / {}",
                    sites.name,
                    trn("count.senders", sites.senders.len() as u64, &[]),
                    trn("count.listeners", sites.listeners.len() as u64, &[])
                );
                egui::CollapsingHeader::new(header)
                    .id_salt(ui.scope_id().with(("who", &pin.name)))
                    .show(ui, |ui| {
                        for (label, list) in [
                            (tr("logic.sent_by"), &sites.senders),
                            (tr("logic.heard_by"), &sites.listeners),
                        ] {
                            ui.label(RichText::new(label).weak());
                            for s in list.iter().take(50) {
                                let text = format!(
                                    "{} #{} · {} {}.{}",
                                    index.streams[s.group.stream]
                                        .rsplit('/')
                                        .next()
                                        .unwrap_or(""),
                                    s.group.resource,
                                    s.object,
                                    s.class_name,
                                    s.pin
                                );
                                if ui
                                    .link(text)
                                    .on_hover_text(&index.streams[s.group.stream])
                                    .clicked()
                                {
                                    jump = Some((
                                        format!("stream:{}", index.streams[s.group.stream]),
                                        (s.group, s.object),
                                    ));
                                }
                            }
                        }
                    });
            }
        }
        if let Some((scope, select)) = jump {
            self.set_scope(&scope, Some(select));
        }
        action
    }

    fn set_value(
        &mut self,
        graph: &LogicGraph,
        n: usize,
        p: usize,
        text: &str,
    ) -> Option<LogicAction> {
        let node = &graph.nodes[n];
        let pin = &node.pins[p];
        let (group, object) = node.object()?;
        let at = edit::AttrAt {
            group,
            object,
            class: pin.class,
            index: pin.index,
        };
        let value = match pin.kind {
            PinKind::In | PinKind::Out => {
                (!text.trim().is_empty()).then(|| Value::String(text.trim().to_owned()))
            }
            _ => {
                let kind = LogicSchema::gf2()
                    .attr(pin.class, pin.index)
                    .and_then(|a| a.kind)
                    .unwrap_or(Kind::Int);
                match parse_value(kind, text) {
                    Ok(v) => v,
                    Err(e) => {
                        self.status = e.to_string();
                        return None;
                    }
                }
            }
        };
        Some(LogicAction::Apply {
            label: trf("logic.op_set", &[("pin", &pin.name)]),
            changes: vec![Change { at, value }],
        })
    }

    fn overview_panel(&mut self, ui: &mut egui::Ui, graph: &LogicGraph) -> Option<LogicAction> {
        let o = graph.overview();
        ui.heading(tr("logic.overview"));
        egui::Grid::new(ui.scope_id().with("overview"))
            .num_columns(2)
            .show(ui, |ui| {
                for (k, v) in [
                    (tr("logic.groups"), o.groups),
                    (tr("logic.nodes"), o.nodes),
                    (tr("logic.wires"), o.wires),
                    (tr("logic.messages"), o.messages),
                ] {
                    ui.label(k);
                    ui.label(v.to_string());
                    ui.end_row();
                }
                for (state, n) in &o.states {
                    ui.label(format!("  {}", state_label(state)));
                    ui.label(n.to_string());
                    ui.end_row();
                }
            });
        ui.separator();
        ui.label(RichText::new(tr("logic.problems")).strong())
            .on_hover_text(tr("logic.problems_hint"));
        let mut dangling: Vec<&logic_graph::Message> = graph.dangling().collect();
        dangling.sort_by_key(|m| (!m.dangling_in_game(), m.name.to_ascii_lowercase()));
        for m in dangling {
            let what = state_text(m.state);
            let color = if m.dangling_in_game() {
                DANGLING_COLOR
            } else {
                Color32::from_gray(170)
            };
            let r = ui.add(
                egui::Label::new(RichText::new(format!("{} — {what}", m.name)).color(color))
                    .sense(egui::Sense::click()),
            );
            let r = match m.elsewhere {
                Some((s, l)) => r.on_hover_text(trf(
                    "logic.elsewhere",
                    &[
                        ("senders", &trn("count.senders", s as u64, &[])),
                        ("listeners", &trn("count.listeners", l as u64, &[])),
                    ],
                )),
                None => r,
            };
            if r.clicked() {
                let ids: Vec<NodeId> = m
                    .senders
                    .iter()
                    .chain(&m.listeners)
                    .map(|p| NodeId(p.node as u64))
                    .collect();
                self.view.set_selection(ids);
                self.view.frame_selection();
            }
        }
        ui.separator();
        ui.label(RichText::new(tr("logic.help")).small().weak());
        None
    }
}

/// What a context menu asked for.
enum MenuCmd {
    Disconnect(usize),
    Clear(PinRef),
    Remove(usize),
    ShowInLevel(usize),
    OpenGroup(usize),
    Add(u32, Pos2),
    Paste(Pos2),
    Comment,
    Layout,
    Frame,
}

fn context_menu(
    ui: &mut egui::Ui,
    graph: &LogicGraph,
    wire_of: &[Option<usize>],
    pins: &HashMap<NodeId, (Vec<PinSource>, Vec<PinSource>)>,
    target: ContextTarget,
    clipboard_empty: bool,
) -> Option<MenuCmd> {
    let mut cmd = None;
    match target {
        ContextTarget::Wire(w) => {
            if let Some(Some(gw)) = wire_of.get(w) {
                if let Some(m) = graph.wires[*gw]
                    .message
                    .and_then(|h| graph.messages.get(&h))
                {
                    ui.label(RichText::new(&m.name).strong());
                }
                if ui.button(tr("logic.disconnect")).clicked() {
                    cmd = Some(MenuCmd::Disconnect(*gw));
                    ui.close();
                }
            }
        }
        ContextTarget::Pin(addr) => {
            let source = pins.get(&addr.node).and_then(|(ins, outs)| {
                (if addr.output { outs } else { ins })
                    .get(addr.pin)
                    .copied()
            });
            if let Some(PinSource::Graph(p)) = source {
                let r = PinRef {
                    node: addr.node.0 as usize,
                    pin: p,
                };
                let pin = &graph.nodes[r.node].pins[p];
                ui.label(RichText::new(&pin.name).strong());
                if (pin.message.is_some() || pin.target.is_some())
                    && matches!(graph.nodes[r.node].source, NodeSource::Object { .. })
                    && ui.button(tr("logic.clear_pin")).clicked()
                {
                    cmd = Some(MenuCmd::Clear(r));
                    ui.close();
                }
            }
        }
        ContextTarget::Node(id) => {
            let n = id.0 as usize;
            if let Some(node) = graph.nodes.get(n) {
                ui.label(RichText::new(node.title()).strong());
                if node.object().is_some() {
                    if node.position.is_some() && ui.button(tr("logic.show_in_level")).clicked() {
                        cmd = Some(MenuCmd::ShowInLevel(n));
                        ui.close();
                    }
                    if ui.button(tr("logic.open_group")).clicked() {
                        cmd = Some(MenuCmd::OpenGroup(n));
                        ui.close();
                    }
                    if ui.button(tr("logic.comment_selection")).clicked() {
                        cmd = Some(MenuCmd::Comment);
                        ui.close();
                    }
                    if ui.button(tr("logic.remove_node")).clicked() {
                        cmd = Some(MenuCmd::Remove(n));
                        ui.close();
                    }
                }
            }
        }
        ContextTarget::Canvas(_) | ContextTarget::Comment(_) => {
            let at = match target {
                ContextTarget::Canvas(p) => p,
                _ => Pos2::ZERO,
            };
            ui.menu_button(tr("logic.add_node"), |ui| {
                let schema = LogicSchema::gf2();
                let classes = schema.classes();
                for cat in [
                    Category::Trigger,
                    Category::Logic,
                    Category::Ui,
                    Category::Audio,
                    Category::Spawn,
                    Category::Job,
                    Category::Cinematic,
                    Category::Relay,
                    Category::Action,
                ] {
                    ui.menu_button(cat.name(), |ui| {
                        egui::ScrollArea::vertical()
                            .max_height(400.0)
                            .show(ui, |ui| {
                                for (h, name, c) in &classes {
                                    if *c == cat && !name.is_empty() && ui.button(*name).clicked() {
                                        cmd = Some(MenuCmd::Add(*h, at));
                                        ui.close();
                                    }
                                }
                            });
                    });
                }
            });
            if !clipboard_empty && ui.button(tr("logic.paste")).clicked() {
                cmd = Some(MenuCmd::Paste(at));
                ui.close();
            }
            if ui.button(tr("logic.comment_selection")).clicked() {
                cmd = Some(MenuCmd::Comment);
                ui.close();
            }
            if ui.button(tr("logic.auto_layout")).clicked() {
                cmd = Some(MenuCmd::Layout);
                ui.close();
            }
            if ui.button(tr("logic.frame")).clicked() {
                cmd = Some(MenuCmd::Frame);
                ui.close();
            }
        }
    }
    cmd
}

fn state_text(state: MessageState) -> &'static str {
    match state {
        MessageState::Connected => tr("logic.state_connected"),
        MessageState::FromEngine => tr("logic.state_from_engine"),
        MessageState::ToEngine => tr("logic.state_to_engine"),
        MessageState::NeverSent => tr("logic.state_never_sent"),
        MessageState::NeverHeard => tr("logic.state_never_heard"),
    }
}

fn state_label(snake: &str) -> &'static str {
    match snake {
        "connected" => state_text(MessageState::Connected),
        "from_engine" => state_text(MessageState::FromEngine),
        "to_engine" => state_text(MessageState::ToEngine),
        "never_sent" => state_text(MessageState::NeverSent),
        _ => state_text(MessageState::NeverHeard),
    }
}

fn pin_help(kind: PinKind, conf: Option<Confidence>) -> String {
    let what = match kind {
        PinKind::In => tr("logic.pin_in"),
        PinKind::Out => tr("logic.pin_out"),
        PinKind::Link => tr("logic.pin_link"),
        PinKind::Asset => tr("logic.pin_asset"),
        PinKind::Param => tr("logic.pin_param"),
    };
    let how = match conf {
        Some(Confidence::Exe) => tr("logic.conf_exe"),
        Some(Confidence::Data) => tr("logic.conf_data"),
        _ => tr("logic.conf_guess"),
    };
    format!("{what} ({how})")
}

/// A file name for a scope.
fn file_key(scope: &str) -> String {
    scope
        .trim()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

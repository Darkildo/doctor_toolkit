//! Drawing and interaction: [`GraphView`].
//!
//! Mouse: left drag on a node moves the selection, on a pin draws a wire,
//! on empty space draws a selection box (Shift adds to the selection);
//! middle or right drag pans; the wheel zooms around the pointer; right
//! click opens the caller's context menu; double click on a node is
//! [`GraphEvent::Activate`], on a comment title edits it. Keys while the
//! pointer is over the canvas: Delete, Ctrl+A, Ctrl+C / Ctrl+V (as copy and
//! paste events), F frames the selection, Home everything.

use std::collections::BTreeSet;

use egui::epaint::CubicBezierShape;
use egui::{
    Align2, Color32, CornerRadius, FontId, Key, PointerButton, Pos2, Rect, Sense, Shape, Stroke,
    StrokeKind, Vec2,
};

use super::metrics as m;
use super::{ANY, Graph, NodeId, WireStyle};

/// A pin of a node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PinAddr {
    pub node: NodeId,
    /// An output (right side) or an input (left side).
    pub output: bool,
    pub pin: usize,
}

/// What a right click or the pointer is on.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ContextTarget {
    /// Empty space at this point of the graph.
    Canvas(Pos2),
    Node(NodeId),
    Pin(PinAddr),
    /// Index into [`Graph::wires`].
    Wire(usize),
    Comment(u64),
}

/// What the user did; the caller changes its model.
#[derive(Debug, Clone, PartialEq)]
pub enum GraphEvent {
    /// A wire was drawn from output `from` to input `to`.
    Connect { from: PinAddr, to: PinAddr },
    /// The wire `Graph::wires[wire]` should go (Delete on a selected wire).
    Disconnect { wire: usize },
    /// Nodes were moved (their `pos` is already updated).
    Moved(Vec<NodeId>),
    /// Delete was pressed with these nodes selected.
    Delete(Vec<NodeId>),
    /// Ctrl+C with these nodes selected.
    Copy(Vec<NodeId>),
    /// Ctrl+V; the pointer was at this point of the graph.
    Paste(Pos2),
    /// Double click on a node.
    Activate(NodeId),
    /// The selection changed.
    Selection(Vec<NodeId>),
    /// Comments were added, moved, resized or renamed.
    CommentsChanged,
}

/// The result of [`GraphView::show`].
pub struct GraphResponse {
    pub events: Vec<GraphEvent>,
    /// What the pointer is over.
    pub hovered: Option<ContextTarget>,
    /// The pointer in graph space, when over the canvas.
    pub pointer: Option<Pos2>,
    pub response: egui::Response,
}

#[derive(Debug, Clone, PartialEq)]
enum Drag {
    None,
    Pan,
    Nodes,
    Box { start: Pos2, add: bool },
    Wire(PinAddr),
    Comment { id: u64, nodes: Vec<NodeId> },
    Resize(u64),
    Minimap,
}

/// Decides whether output `from` may be wired to input `to`.
pub type ConnectRule = Box<dyn Fn(&Graph, PinAddr, PinAddr) -> bool>;

/// The state of a node-graph widget: view transform, selection, dragging.
pub struct GraphView {
    /// Screen offset of the graph origin from the canvas corner.
    pub pan: Vec2,
    pub zoom: f32,
    pub show_minimap: bool,
    /// Extra rule for new wires (beyond matching pin types).
    pub can_connect: Option<ConnectRule>,
    selected: BTreeSet<NodeId>,
    selected_wire: Option<usize>,
    drag: Drag,
    context: Option<ContextTarget>,
    frame: Option<bool>,
    editing: Option<(u64, String)>,
    moved: bool,
}

impl Default for GraphView {
    fn default() -> Self {
        Self {
            pan: Vec2::new(40.0, 40.0),
            zoom: 1.0,
            show_minimap: true,
            can_connect: None,
            selected: BTreeSet::new(),
            selected_wire: None,
            drag: Drag::None,
            context: None,
            frame: None,
            editing: None,
            moved: false,
        }
    }
}

const MIN_ZOOM: f32 = 0.08;
const MAX_ZOOM: f32 = 2.5;
const MINIMAP: Vec2 = Vec2::new(200.0, 140.0);

impl GraphView {
    pub fn selection(&self) -> &BTreeSet<NodeId> {
        &self.selected
    }

    pub fn set_selection(&mut self, ids: impl IntoIterator<Item = NodeId>) {
        self.selected = ids.into_iter().collect();
        self.selected_wire = None;
    }

    pub fn selected_wire(&self) -> Option<usize> {
        self.selected_wire
    }

    /// Frames everything on the next frame.
    pub fn frame_all(&mut self) {
        self.frame = Some(false);
    }

    /// Frames the selection on the next frame.
    pub fn frame_selection(&mut self) {
        self.frame = Some(true);
    }

    /// Screen position of point `p` of the graph on `canvas` (the rect of
    /// the last [`Self::show`]'s response) — for layers drawn over the
    /// widget.
    pub fn graph_to_screen(&self, canvas: Rect, p: Pos2) -> Pos2 {
        self.to_screen(canvas, p)
    }

    /// Screen points of the wire `from` → `to` as the widget draws it.
    pub fn wire_screen_points(
        &self,
        graph: &Graph,
        canvas: Rect,
        from: (NodeId, usize),
        to: (NodeId, usize),
    ) -> Option<Vec<Pos2>> {
        self.wire_points(graph, canvas, from, to)
    }

    fn to_screen(&self, canvas: Rect, p: Pos2) -> Pos2 {
        canvas.min + self.pan + p.to_vec2() * self.zoom
    }

    fn to_graph(&self, canvas: Rect, s: Pos2) -> Pos2 {
        ((s - canvas.min - self.pan) / self.zoom).to_pos2()
    }

    fn fit(&mut self, canvas: Rect, bounds: Rect) {
        let bounds = bounds.expand(40.0);
        let zoom = (canvas.width() / bounds.width())
            .min(canvas.height() / bounds.height())
            .clamp(MIN_ZOOM, 1.2);
        self.zoom = zoom;
        self.pan = canvas.size() / 2.0 - bounds.center().to_vec2() * zoom;
    }

    /// Centres the view on a point of the graph.
    pub fn center_on(&mut self, canvas_size: Vec2, p: Pos2) {
        self.pan = canvas_size / 2.0 - p.to_vec2() * self.zoom;
    }

    fn connectable(&self, graph: &Graph, a: PinAddr, b: PinAddr) -> Option<(PinAddr, PinAddr)> {
        if a.output == b.output || a.node == b.node {
            return None;
        }
        let (from, to) = if a.output { (a, b) } else { (b, a) };
        let ty = |p: PinAddr| {
            let n = graph.node(p.node)?;
            let pins = if p.output { &n.outputs } else { &n.inputs };
            pins.get(p.pin).map(|x| x.ty)
        };
        let (tf, tt) = (ty(from)?, ty(to)?);
        if tf != tt && tf != ANY && tt != ANY {
            return None;
        }
        if let Some(rule) = &self.can_connect
            && !rule(graph, from, to)
        {
            return None;
        }
        Some((from, to))
    }

    /// What is at screen point `s`.
    fn hit(&self, graph: &Graph, canvas: Rect, s: Pos2) -> Option<ContextTarget> {
        let g = self.to_graph(canvas, s);
        let pin_r = (m::PIN_RADIUS + 4.0) / self.zoom.max(0.3);
        // Pins and nodes, the last drawn first.
        for n in graph.nodes.iter().rev() {
            let r = n.rect();
            if !r.expand(pin_r).contains(g) {
                continue;
            }
            for (output, pins) in [(false, &n.inputs), (true, &n.outputs)] {
                for i in 0..pins.len() {
                    if n.pin_pos(output, i).distance(g) <= pin_r {
                        return Some(ContextTarget::Pin(PinAddr {
                            node: n.id,
                            output,
                            pin: i,
                        }));
                    }
                }
            }
            if r.contains(g) {
                return Some(ContextTarget::Node(n.id));
            }
        }
        for c in graph.comments.iter().rev() {
            let bar = Rect::from_min_size(c.rect.min, Vec2::new(c.rect.width(), m::COMMENT_BAR));
            if bar.contains(g) {
                return Some(ContextTarget::Comment(c.id));
            }
        }
        let tolerance = 6.0;
        for (i, w) in graph.wires.iter().enumerate() {
            if let Some(points) = self.wire_points(graph, canvas, w.from, w.to)
                && polyline_distance(&points, s) <= tolerance
            {
                return Some(ContextTarget::Wire(i));
            }
        }
        for c in graph.comments.iter().rev() {
            if c.rect.contains(g) {
                return Some(ContextTarget::Comment(c.id));
            }
        }
        None
    }

    fn wire_points(
        &self,
        graph: &Graph,
        canvas: Rect,
        from: (NodeId, usize),
        to: (NodeId, usize),
    ) -> Option<Vec<Pos2>> {
        let a = graph.node(from.0)?.pin_pos(true, from.1);
        let b = graph.node(to.0)?.pin_pos(false, to.1);
        Some(bezier_points(
            self.to_screen(canvas, a),
            self.to_screen(canvas, b),
            self.zoom,
        ))
    }

    /// Draws the graph and handles input. `menu` fills the context menu of a
    /// right click on its target.
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        graph: &mut Graph,
        menu: &mut dyn FnMut(&mut egui::Ui, ContextTarget),
    ) -> GraphResponse {
        let (canvas, response) =
            ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
        let painter = ui.painter_at(canvas);
        let mut events = Vec::new();
        if let Some(selection) = self.frame.take() {
            let bounds = if selection {
                graph
                    .nodes
                    .iter()
                    .filter(|n| self.selected.contains(&n.id))
                    .map(|n| n.rect())
                    .reduce(|a, b| a.union(b))
            } else {
                None
            };
            if let Some(b) = bounds.or_else(|| graph.bounds()) {
                self.fit(canvas, b);
            }
        }
        let pointer = ui
            .input(|i| i.pointer.hover_pos())
            .filter(|p| canvas.contains(*p));
        let minimap = self.minimap_rect(canvas, graph);
        // Zoom around the pointer.
        if response.hovered()
            && let Some(p) = pointer
        {
            let (scroll, pinch) = ui.input(|i| (i.smooth_scroll_delta.y, i.zoom_delta()));
            let factor = pinch * (scroll * 0.0015).exp();
            if (factor - 1.0).abs() > 1e-4 {
                let before = self.to_graph(canvas, p);
                self.zoom = (self.zoom * factor).clamp(MIN_ZOOM, MAX_ZOOM);
                self.pan = p - canvas.min - before.to_vec2() * self.zoom;
            }
            if scroll != 0.0 {
                ui.ctx()
                    .input_mut(|i| i.smooth_scroll_delta = egui::Vec2::ZERO);
            }
        }
        let hovered = pointer.and_then(|p| self.hit(graph, canvas, p));

        // Drags.
        let press = ui.input(|i| i.pointer.press_origin());
        let shift = ui.input(|i| i.modifiers.shift);
        let ctrl = ui.input(|i| i.modifiers.command);
        if response.drag_started_by(PointerButton::Middle)
            || response.drag_started_by(PointerButton::Secondary)
        {
            self.drag = Drag::Pan;
        } else if response.drag_started_by(PointerButton::Primary)
            && let Some(start) = press
        {
            self.drag = if minimap.is_some_and(|r| r.contains(start)) {
                Drag::Minimap
            } else {
                match self.hit(graph, canvas, start) {
                    Some(ContextTarget::Pin(p)) => Drag::Wire(p),
                    Some(ContextTarget::Node(id)) => {
                        if !self.selected.contains(&id) {
                            if !(shift || ctrl) {
                                self.selected.clear();
                            }
                            self.selected.insert(id);
                            events.push(GraphEvent::Selection(
                                self.selected.iter().copied().collect(),
                            ));
                        }
                        self.moved = false;
                        Drag::Nodes
                    }
                    Some(ContextTarget::Comment(id)) => {
                        let g = self.to_graph(canvas, start);
                        let c = graph.comments.iter().find(|c| c.id == id).expect("hit");
                        let corner = c.rect.max - Vec2::splat(14.0 / self.zoom.max(0.3));
                        if g.x >= corner.x && g.y >= corner.y {
                            Drag::Resize(id)
                        } else if g.y <= c.rect.min.y + m::COMMENT_BAR {
                            let nodes = graph
                                .nodes
                                .iter()
                                .filter(|n| c.rect.contains_rect(n.rect()))
                                .map(|n| n.id)
                                .collect();
                            Drag::Comment { id, nodes }
                        } else {
                            Drag::Box { start, add: shift }
                        }
                    }
                    _ => Drag::Box { start, add: shift },
                }
            };
        }
        let delta = response.drag_delta() / self.zoom;
        if response.dragged() {
            match &self.drag {
                Drag::Pan => self.pan += response.drag_delta(),
                Drag::Nodes => {
                    for n in &mut graph.nodes {
                        if self.selected.contains(&n.id) {
                            n.pos += delta;
                        }
                    }
                    self.moved |= delta != Vec2::ZERO;
                }
                Drag::Comment { id, nodes } => {
                    if let Some(c) = graph.comments.iter_mut().find(|c| c.id == *id) {
                        c.rect = c.rect.translate(delta);
                    }
                    for n in &mut graph.nodes {
                        if nodes.contains(&n.id) {
                            n.pos += delta;
                        }
                    }
                    self.moved |= delta != Vec2::ZERO;
                }
                Drag::Resize(id) => {
                    if let Some(c) = graph.comments.iter_mut().find(|c| c.id == *id) {
                        c.rect.max += delta;
                        c.rect.max.x = c.rect.max.x.max(c.rect.min.x + 80.0);
                        c.rect.max.y = c.rect.max.y.max(c.rect.min.y + 40.0);
                    }
                }
                Drag::Minimap => {
                    if let (Some(r), Some(p), Some(b)) =
                        (minimap, pointer, minimap_bounds(self, canvas, graph))
                    {
                        let t = (p - r.min) / r.size();
                        let g = b.min + b.size() * t;
                        self.center_on(canvas.size(), g);
                    }
                }
                _ => {}
            }
        }
        if response.drag_stopped() {
            match std::mem::replace(&mut self.drag, Drag::None) {
                Drag::Nodes if self.moved => {
                    events.push(GraphEvent::Moved(self.selected.iter().copied().collect()));
                }
                Drag::Comment { nodes, .. } => {
                    if !nodes.is_empty() {
                        events.push(GraphEvent::Moved(nodes));
                    }
                    events.push(GraphEvent::CommentsChanged);
                }
                Drag::Resize(_) => events.push(GraphEvent::CommentsChanged),
                Drag::Wire(a) => {
                    if let Some(ContextTarget::Pin(b)) = hovered
                        && let Some((from, to)) = self.connectable(graph, a, b)
                    {
                        events.push(GraphEvent::Connect { from, to });
                    }
                }
                Drag::Box { start, add } => {
                    if let Some(end) = pointer.or(ui.input(|i| i.pointer.interact_pos())) {
                        let r = Rect::from_two_pos(
                            self.to_graph(canvas, start),
                            self.to_graph(canvas, end),
                        );
                        if !add {
                            self.selected.clear();
                        }
                        for n in &graph.nodes {
                            if r.intersects(n.rect()) {
                                self.selected.insert(n.id);
                            }
                        }
                        self.selected_wire = None;
                        events.push(GraphEvent::Selection(
                            self.selected.iter().copied().collect(),
                        ));
                    }
                }
                _ => {}
            }
        }

        // Clicks.
        if response.clicked() {
            response.request_focus();
            match hovered {
                Some(ContextTarget::Node(id))
                | Some(ContextTarget::Pin(PinAddr { node: id, .. })) => {
                    if shift || ctrl {
                        if !self.selected.remove(&id) {
                            self.selected.insert(id);
                        }
                    } else {
                        self.selected = BTreeSet::from([id]);
                    }
                    self.selected_wire = None;
                }
                Some(ContextTarget::Wire(i)) => {
                    self.selected.clear();
                    self.selected_wire = Some(i);
                }
                _ => {
                    self.selected.clear();
                    self.selected_wire = None;
                }
            }
            events.push(GraphEvent::Selection(
                self.selected.iter().copied().collect(),
            ));
        }
        if response.double_clicked() {
            match hovered {
                Some(ContextTarget::Node(id)) => events.push(GraphEvent::Activate(id)),
                Some(ContextTarget::Comment(id)) => {
                    let text = graph
                        .comments
                        .iter()
                        .find(|c| c.id == id)
                        .map(|c| c.text.clone())
                        .unwrap_or_default();
                    self.editing = Some((id, text));
                }
                _ => {}
            }
        }
        if response.secondary_clicked() {
            self.context = Some(match hovered {
                Some(t) => t,
                None => {
                    ContextTarget::Canvas(self.to_graph(canvas, pointer.unwrap_or(canvas.center())))
                }
            });
            if let Some(ContextTarget::Node(id)) = self.context
                && !self.selected.contains(&id)
            {
                self.selected = BTreeSet::from([id]);
                events.push(GraphEvent::Selection(vec![id]));
            }
        }
        if let Some(target) = self.context {
            response.context_menu(|ui| menu(ui, target));
        }

        // Keys.
        if response.hovered() && !ui.ctx().egui_wants_keyboard_input() && self.editing.is_none() {
            let (del, all, frame, home, copy, paste) = ui.input(|i| {
                (
                    i.key_pressed(Key::Delete),
                    i.modifiers.command && i.key_pressed(Key::A),
                    !i.modifiers.any() && i.key_pressed(Key::F),
                    i.key_pressed(Key::Home),
                    i.events.iter().any(|e| matches!(e, egui::Event::Copy)),
                    i.events.iter().any(|e| matches!(e, egui::Event::Paste(_)))
                        || (i.modifiers.command && i.key_pressed(Key::V)),
                )
            });
            if del {
                if let Some(w) = self.selected_wire.take() {
                    events.push(GraphEvent::Disconnect { wire: w });
                } else if !self.selected.is_empty() {
                    events.push(GraphEvent::Delete(self.selected.iter().copied().collect()));
                }
            }
            if all {
                self.selected = graph.nodes.iter().map(|n| n.id).collect();
                events.push(GraphEvent::Selection(
                    self.selected.iter().copied().collect(),
                ));
            }
            if frame {
                self.frame = Some(!self.selected.is_empty());
            }
            if home {
                self.frame = Some(false);
            }
            if copy && !self.selected.is_empty() {
                events.push(GraphEvent::Copy(self.selected.iter().copied().collect()));
            }
            if paste {
                let at = self.to_graph(canvas, pointer.unwrap_or(canvas.center()));
                events.push(GraphEvent::Paste(at));
            }
        }

        // Drawing.
        self.draw_grid(&painter, canvas);
        let visible = Rect::from_two_pos(
            self.to_graph(canvas, canvas.min),
            self.to_graph(canvas, canvas.max),
        );
        self.draw_comments(ui, &painter, canvas, graph, &mut events);
        let hovered_wire = match hovered {
            Some(ContextTarget::Wire(i)) => Some(i),
            _ => None,
        };
        let related = |id: NodeId| self.selected.contains(&id);
        for (i, w) in graph.wires.iter().enumerate() {
            let (Some(a), Some(b)) = (graph.node(w.from.0), graph.node(w.to.0)) else {
                continue;
            };
            let pa = a.pin_pos(true, w.from.1);
            let pb = b.pin_pos(false, w.to.1);
            if !visible.expand(200.0).intersects(Rect::from_two_pos(pa, pb)) {
                continue;
            }
            let hot = hovered_wire == Some(i)
                || self.selected_wire == Some(i)
                || related(w.from.0)
                || related(w.to.0);
            let width = if hot { 3.0 } else { 1.6 } * self.zoom.clamp(0.5, 1.5);
            let color = if a.ghost || b.ghost {
                w.color.gamma_multiply(0.4)
            } else if hot {
                w.color
            } else {
                w.color.gamma_multiply(0.8)
            };
            draw_wire(
                &painter,
                self.to_screen(canvas, pa),
                self.to_screen(canvas, pb),
                self.zoom,
                Stroke::new(width, color),
                w.style,
            );
        }
        if let Drag::Wire(p) = &self.drag
            && let (Some(n), Some(end)) = (graph.node(p.node), pointer)
        {
            let start = self.to_screen(canvas, n.pin_pos(p.output, p.pin));
            let (a, b) = if p.output { (start, end) } else { (end, start) };
            let ok = matches!(hovered, Some(ContextTarget::Pin(q)) if self.connectable(graph, *p, q).is_some());
            let color = if ok {
                Color32::LIGHT_GREEN
            } else {
                Color32::from_gray(200)
            };
            draw_wire(
                &painter,
                a,
                b,
                self.zoom,
                Stroke::new(2.0, color),
                WireStyle::Dashed,
            );
        }
        let hover_node = match hovered {
            Some(ContextTarget::Node(id)) => Some(id),
            _ => None,
        };
        for n in &graph.nodes {
            if !visible.intersects(n.rect()) {
                continue;
            }
            self.draw_node(
                &painter,
                canvas,
                n,
                self.selected.contains(&n.id),
                hover_node == Some(n.id),
            );
        }
        if let Drag::Box { start, .. } = &self.drag
            && let Some(end) = pointer
        {
            let r = Rect::from_two_pos(*start, end);
            painter.rect(
                r,
                0.0,
                Color32::from_rgba_unmultiplied(120, 160, 255, 30),
                Stroke::new(1.0, Color32::from_rgb(120, 160, 255)),
                StrokeKind::Inside,
            );
        }
        if let Some(r) = minimap {
            self.draw_minimap(&painter, canvas, r, graph);
        }
        // Tooltips.
        match hovered {
            Some(ContextTarget::Pin(p)) => {
                let tip = graph.node(p.node).and_then(|n| {
                    let pins = if p.output { &n.outputs } else { &n.inputs };
                    pins.get(p.pin)
                        .map(|x| x.tooltip.clone().unwrap_or_else(|| x.label.clone()))
                });
                if let Some(tip) = tip {
                    response.clone().on_hover_text_at_pointer(tip);
                }
            }
            Some(ContextTarget::Wire(i)) => {
                if let Some(label) = graph.wires.get(i).and_then(|w| w.label.clone()) {
                    response.clone().on_hover_text_at_pointer(label);
                }
            }
            Some(ContextTarget::Node(id)) => {
                if let Some(n) = graph.node(id)
                    && let Some(b) = n.badges.first()
                    && self.zoom < 0.5
                {
                    response.clone().on_hover_text_at_pointer(b.tooltip.clone());
                }
            }
            _ => {}
        }
        GraphResponse {
            events,
            hovered,
            pointer: pointer.map(|p| self.to_graph(canvas, p)),
            response,
        }
    }

    fn draw_grid(&self, painter: &egui::Painter, canvas: Rect) {
        painter.rect_filled(canvas, 0.0, Color32::from_gray(28));
        let step = 32.0 * self.zoom;
        if step < 8.0 {
            return;
        }
        let color = Color32::from_gray(36);
        let origin = canvas.min + self.pan;
        let mut x = canvas.min.x + (origin.x - canvas.min.x).rem_euclid(step);
        while x < canvas.max.x {
            painter.line_segment(
                [Pos2::new(x, canvas.min.y), Pos2::new(x, canvas.max.y)],
                Stroke::new(1.0, color),
            );
            x += step;
        }
        let mut y = canvas.min.y + (origin.y - canvas.min.y).rem_euclid(step);
        while y < canvas.max.y {
            painter.line_segment(
                [Pos2::new(canvas.min.x, y), Pos2::new(canvas.max.x, y)],
                Stroke::new(1.0, color),
            );
            y += step;
        }
    }

    fn draw_comments(
        &mut self,
        ui: &mut egui::Ui,
        painter: &egui::Painter,
        canvas: Rect,
        graph: &mut Graph,
        events: &mut Vec<GraphEvent>,
    ) {
        for c in &graph.comments {
            let r = Rect::from_min_max(
                self.to_screen(canvas, c.rect.min),
                self.to_screen(canvas, c.rect.max),
            );
            if !r.intersects(canvas) {
                continue;
            }
            painter.rect(
                r,
                4.0,
                c.color,
                Stroke::new(1.0, c.color.gamma_multiply(2.0)),
                StrokeKind::Inside,
            );
            let bar = Rect::from_min_size(r.min, Vec2::new(r.width(), m::COMMENT_BAR * self.zoom));
            painter.rect_filled(
                bar,
                CornerRadius {
                    nw: 4,
                    ne: 4,
                    sw: 0,
                    se: 0,
                },
                c.color.gamma_multiply(2.0),
            );
            if self.zoom > 0.25 && self.editing.as_ref().is_none_or(|(id, _)| *id != c.id) {
                painter.text(
                    bar.left_center() + Vec2::new(6.0, 0.0),
                    Align2::LEFT_CENTER,
                    &c.text,
                    FontId::proportional((13.0 * self.zoom).max(6.0)),
                    Color32::WHITE,
                );
            }
            // Resize handle.
            let h = r.max - Vec2::splat(10.0);
            painter.line_segment(
                [h + Vec2::new(0.0, 8.0), h + Vec2::new(8.0, 0.0)],
                Stroke::new(1.0, Color32::from_gray(200)),
            );
        }
        if let Some((id, mut text)) = self.editing.take() {
            let Some(c) = graph.comments.iter_mut().find(|c| c.id == id) else {
                return;
            };
            let r = Rect::from_min_size(
                self.to_screen(canvas, c.rect.min),
                Vec2::new((c.rect.width() * self.zoom).max(120.0), 22.0),
            );
            let edit = ui.put(r, egui::TextEdit::singleline(&mut text));
            edit.request_focus();
            if edit.lost_focus() || ui.input(|i| i.key_pressed(Key::Enter)) {
                c.text = text;
                events.push(GraphEvent::CommentsChanged);
            } else {
                self.editing = Some((id, text));
            }
        }
    }

    fn draw_node(
        &self,
        painter: &egui::Painter,
        canvas: Rect,
        n: &super::Node,
        selected: bool,
        hovered: bool,
    ) {
        let z = self.zoom;
        let r = Rect::from_min_max(
            self.to_screen(canvas, n.rect().min),
            self.to_screen(canvas, n.rect().max),
        );
        let alpha = if n.ghost { 0.45 } else { 1.0 };
        let body = Color32::from_gray(44).gamma_multiply(alpha);
        let outline = if selected {
            Stroke::new(2.0, Color32::from_rgb(255, 200, 80))
        } else if hovered {
            Stroke::new(1.5, Color32::from_gray(160))
        } else {
            Stroke::new(1.0, Color32::from_gray(70))
        };
        let radius = (6.0 * z).clamp(1.0, 8.0) as u8;
        painter.rect(r, radius, body, outline, StrokeKind::Outside);
        let header = Rect::from_min_size(r.min, Vec2::new(r.width(), m::HEADER * z));
        painter.rect_filled(
            header,
            CornerRadius {
                nw: radius,
                ne: radius,
                sw: 0,
                se: 0,
            },
            n.header_color.gamma_multiply(alpha),
        );
        let text = z >= 0.35;
        if text {
            let title_font = FontId::proportional(13.0 * z);
            let small = FontId::proportional(10.5 * z);
            let mut badge_x = header.max.x - 4.0 * z;
            for b in n.badges.iter().rev() {
                let galley = painter.layout_no_wrap(b.text.clone(), small.clone(), Color32::BLACK);
                let w = galley.size().x + 8.0 * z;
                let br = Rect::from_min_size(
                    Pos2::new(badge_x - w, header.min.y + 4.0 * z),
                    Vec2::new(w, 14.0 * z),
                );
                painter.rect_filled(br, 3.0 * z, b.color);
                painter.galley(
                    br.min + Vec2::new(4.0 * z, (br.height() - galley.size().y) / 2.0),
                    galley,
                    Color32::BLACK,
                );
                badge_x -= w + 3.0 * z;
            }
            let clip = painter.with_clip_rect(
                Rect::from_min_max(header.min, Pos2::new(badge_x, header.max.y)).intersect(canvas),
            );
            clip.text(
                header.min + Vec2::new(8.0 * z, 4.0 * z),
                Align2::LEFT_TOP,
                &n.title,
                title_font,
                Color32::WHITE.gamma_multiply(alpha),
            );
            if !n.subtitle.is_empty() {
                clip.text(
                    header.min + Vec2::new(8.0 * z, 19.0 * z),
                    Align2::LEFT_TOP,
                    &n.subtitle,
                    small.clone(),
                    Color32::from_gray(200).gamma_multiply(alpha),
                );
            }
        }
        let label_font = FontId::proportional(11.0 * z);
        let half = r.width() / 2.0 - 10.0 * z;
        for (output, pins) in [(false, &n.inputs), (true, &n.outputs)] {
            for (i, p) in pins.iter().enumerate() {
                let c = self.to_screen(canvas, n.pin_pos(output, i));
                let rad = (m::PIN_RADIUS * z).max(1.5);
                let color = p.color.gamma_multiply(alpha);
                if p.set {
                    painter.circle_filled(c, rad, color);
                } else {
                    painter.circle_stroke(c, rad, Stroke::new(1.5, color));
                }
                if text {
                    let (pos, align) = if output {
                        (c - Vec2::new(10.0 * z, 0.0), Align2::RIGHT_CENTER)
                    } else {
                        (c + Vec2::new(10.0 * z, 0.0), Align2::LEFT_CENTER)
                    };
                    let clip_rect = if output {
                        Rect::from_min_max(
                            Pos2::new(r.max.x - half - 10.0 * z, c.y - 8.0 * z),
                            Pos2::new(r.max.x, c.y + 8.0 * z),
                        )
                    } else {
                        Rect::from_min_max(
                            Pos2::new(r.min.x, c.y - 8.0 * z),
                            Pos2::new(r.min.x + half + 10.0 * z, c.y + 8.0 * z),
                        )
                    };
                    let fit = ((half - 4.0 * z) / (6.2 * z)).max(3.0) as usize;
                    let label = ellipsize(&p.label, fit);
                    painter.with_clip_rect(clip_rect.intersect(canvas)).text(
                        pos,
                        align,
                        label,
                        label_font.clone(),
                        Color32::from_gray(220).gamma_multiply(alpha),
                    );
                }
            }
        }
        if text {
            let rows = n.inputs.len().max(n.outputs.len());
            for (k, line) in n.lines.iter().enumerate() {
                let y = r.min.y + (m::HEADER + (rows + k) as f32 * m::ROW + 2.0) * z;
                painter.with_clip_rect(r.intersect(canvas)).text(
                    Pos2::new(r.min.x + 8.0 * z, y),
                    Align2::LEFT_TOP,
                    line,
                    label_font.clone(),
                    Color32::from_gray(170).gamma_multiply(alpha),
                );
            }
        }
    }

    fn minimap_rect(&self, canvas: Rect, graph: &Graph) -> Option<Rect> {
        if !self.show_minimap || graph.nodes.is_empty() || canvas.width() < 400.0 {
            return None;
        }
        Some(Rect::from_min_size(
            canvas.max - MINIMAP - Vec2::splat(10.0),
            MINIMAP,
        ))
    }

    fn draw_minimap(&self, painter: &egui::Painter, canvas: Rect, r: Rect, graph: &Graph) {
        let Some(b) = minimap_bounds(self, canvas, graph) else {
            return;
        };
        painter.rect(
            r,
            4.0,
            Color32::from_black_alpha(170),
            Stroke::new(1.0, Color32::from_gray(90)),
            StrokeKind::Inside,
        );
        let map = |p: Pos2| r.min + (p - b.min) / b.size() * r.size();
        for n in &graph.nodes {
            let nr = Rect::from_min_max(map(n.rect().min), map(n.rect().max));
            let color = if self.selected.contains(&n.id) {
                Color32::from_rgb(255, 200, 80)
            } else {
                n.header_color
            };
            painter.rect_filled(nr.expand(0.5), 0.0, color);
        }
        let view = Rect::from_two_pos(
            self.to_graph(canvas, canvas.min),
            self.to_graph(canvas, canvas.max),
        );
        painter.rect_stroke(
            Rect::from_min_max(map(view.min), map(view.max)).intersect(r),
            0.0,
            Stroke::new(1.0, Color32::WHITE),
            StrokeKind::Inside,
        );
    }
}

/// The area the minimap shows: the graph and the current view.
fn minimap_bounds(view: &GraphView, canvas: Rect, graph: &Graph) -> Option<Rect> {
    let g = graph.bounds()?;
    let v = Rect::from_two_pos(
        view.to_graph(canvas, canvas.min),
        view.to_graph(canvas, canvas.max),
    );
    let b = g.union(v).expand(20.0);
    // Keep the aspect of the minimap.
    let aspect = MINIMAP.x / MINIMAP.y;
    let (w, h) = (b.width(), b.height());
    Some(if w / h > aspect {
        Rect::from_center_size(b.center(), Vec2::new(w, w / aspect))
    } else {
        Rect::from_center_size(b.center(), Vec2::new(h * aspect, h))
    })
}

fn control_points(a: Pos2, b: Pos2, zoom: f32) -> [Pos2; 4] {
    let dx = ((b.x - a.x).abs() * 0.5).max(40.0 * zoom);
    [a, a + Vec2::new(dx, 0.0), b - Vec2::new(dx, 0.0), b]
}

fn bezier_points(a: Pos2, b: Pos2, zoom: f32) -> Vec<Pos2> {
    let [p0, p1, p2, p3] = control_points(a, b, zoom);
    (0..=20)
        .map(|i| {
            let t = i as f32 / 20.0;
            let u = 1.0 - t;
            (p0.to_vec2() * (u * u * u)
                + p1.to_vec2() * (3.0 * u * u * t)
                + p2.to_vec2() * (3.0 * u * t * t)
                + p3.to_vec2() * (t * t * t))
                .to_pos2()
        })
        .collect()
}

fn draw_wire(
    painter: &egui::Painter,
    a: Pos2,
    b: Pos2,
    zoom: f32,
    stroke: Stroke,
    style: WireStyle,
) {
    match style {
        WireStyle::Solid => {
            painter.add(Shape::CubicBezier(CubicBezierShape::from_points_stroke(
                control_points(a, b, zoom),
                false,
                Color32::TRANSPARENT,
                stroke,
            )));
        }
        WireStyle::Dashed | WireStyle::Dotted => {
            let points = bezier_points(a, b, zoom);
            let (dash, gap) = if style == WireStyle::Dashed {
                (8.0, 5.0)
            } else {
                (2.0, 4.0)
            };
            painter.extend(Shape::dashed_line(&points, stroke, dash, gap));
        }
    }
}

/// `text` cut to `max` characters with an ellipsis.
fn ellipsize(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let mut out: String = text.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// Distance from `p` to a polyline.
pub(super) fn polyline_distance(points: &[Pos2], p: Pos2) -> f32 {
    points
        .windows(2)
        .map(|s| {
            let (a, b) = (s[0], s[1]);
            let ab = b - a;
            let t = ((p - a).dot(ab) / ab.length_sq().max(1e-6)).clamp(0.0, 1.0);
            (a + ab * t).distance(p)
        })
        .fold(f32::INFINITY, f32::min)
}

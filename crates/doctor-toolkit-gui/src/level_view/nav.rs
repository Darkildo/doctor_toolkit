//! Navigation graphs (`graph`) in the level view: drawn as lines over the
//! scene, coloured by graph type; nodes are picked, moved with the gizmo,
//! added, linked and removed. Every edit is one [`LevelAction::Graph`] that
//! the app records in the workspace (undo, diff and apply as for other edits).

use doctor_toolkit::game::GameInstance;
use doctor_toolkit::level::navgraph::{GraphRef, graphs_of_streams};
use doctor_toolkit::resource::graph::{
    Graph, Neighbourhood, flag_names, graph_flags, link_flags, node_flags,
};
use doctor_toolkit::workspace::Workspace;
use doctor_toolkit_mods::i18n::{tr, trf};
use egui::RichText;

use super::LevelAction;
use crate::render::LineVertex;

/// A node of a loaded graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NodeRef {
    pub graph: usize,
    pub node: usize,
}

/// Lines are drawn this far above the nodes, over the ground they lie on.
const LIFT: f32 = 0.15;
/// A click selects a node this close on the screen (points).
pub const PICK_RADIUS: f32 = 9.0;

const SELECTED: [u8; 4] = [255, 230, 40, 255];
const SPECIAL: [u8; 4] = [60, 60, 255, 255];
const CROSSWALK: [u8; 4] = [240, 240, 240, 255];
const DISABLED: [u8; 4] = [20, 20, 20, 255];

/// Colour of a graph by its type.
fn type_color(flags: u16) -> [u8; 4] {
    if flags & graph_flags::VEHICULAR_PARKING != 0 {
        [190, 90, 230, 255]
    } else if flags & graph_flags::VEHICULAR != 0 {
        [70, 150, 255, 255]
    } else if flags & graph_flags::PATROL != 0 {
        [255, 140, 30, 255]
    } else if flags & graph_flags::AUDIO_OCCLUSION != 0 {
        [150, 150, 150, 255]
    } else if flags & graph_flags::PED != 0 {
        [40, 220, 220, 255]
    } else {
        [70, 220, 70, 255]
    }
}

pub struct NavLayer {
    /// Draw the graphs.
    pub visible: bool,
    /// Graph type to show (0: all).
    pub layer: u16,
    /// Clicks pick nodes before objects.
    pub edit: bool,
    graphs: Vec<GraphRef>,
    names: Vec<String>,
    failed: Vec<String>,
    /// Streams the graphs were read from.
    streams: Vec<usize>,
    pub selected: Option<NodeRef>,
    /// Position of a node being dragged.
    pub preview: Option<(NodeRef, [f32; 3])>,
    two_way: bool,
    how: Neighbourhood,
    confirm_remove: bool,
    /// Changes when the drawn lines change.
    pub generation: u64,
}

impl Default for NavLayer {
    fn default() -> Self {
        Self {
            visible: false,
            layer: 0,
            edit: true,
            graphs: Vec::new(),
            names: Vec::new(),
            failed: Vec::new(),
            streams: Vec::new(),
            selected: None,
            preview: None,
            two_way: true,
            how: Neighbourhood::default(),
            confirm_remove: false,
            generation: 0,
        }
    }
}

impl NavLayer {
    /// Reads the graphs of `streams` (again when the workspace changed).
    pub fn load(&mut self, game: &GameInstance, ws: Option<&Workspace>, streams: &[usize]) {
        // Indices mean other nodes in other streams.
        let keep = self
            .selected
            .filter(|_| self.streams.is_empty() || self.streams == streams);
        match graphs_of_streams(game, ws, streams) {
            Ok((graphs, failed)) => {
                self.names = graphs
                    .iter()
                    .map(|g| format!("{} #{}", game.streams()[g.stream].stream_name(), g.resource))
                    .collect();
                self.graphs = graphs;
                self.failed = failed;
            }
            Err(error) => {
                self.graphs.clear();
                self.names.clear();
                self.failed = vec![error.to_string()];
            }
        }
        self.streams = streams.to_vec();
        self.selected = keep.filter(|r| self.position(*r).is_some());
        self.preview = None;
        self.generation += 1;
    }

    /// Loads the graphs if they are shown and were read for other streams.
    pub fn ensure(&mut self, game: &GameInstance, ws: Option<&Workspace>, streams: &[usize]) {
        if self.visible && self.streams != streams {
            self.load(game, ws, streams);
        }
    }

    /// Reads the graphs again after an edit, undo or apply.
    pub fn reload(&mut self, game: &GameInstance, ws: Option<&Workspace>) {
        if !self.streams.is_empty() {
            let streams = self.streams.clone();
            self.load(game, ws, &streams);
        }
    }

    fn shown(&self, g: usize) -> bool {
        self.layer == 0 || self.graphs[g].graph.flags & self.layer != 0
    }

    /// Position of a node, with the drag preview.
    pub fn position(&self, r: NodeRef) -> Option<[f32; 3]> {
        if let Some((p, at)) = self.preview
            && p == r
        {
            return Some(at);
        }
        Some(self.graphs.get(r.graph)?.graph.nodes.get(r.node)?.position)
    }

    /// The lines to draw.
    pub fn lines(&self) -> Vec<LineVertex> {
        let mut out = Vec::new();
        if !self.visible {
            return out;
        }
        let lift = |p: [f32; 3]| [p[0], p[1] + LIFT, p[2]];
        for (g, gr) in self.graphs.iter().enumerate() {
            if !self.shown(g) {
                continue;
            }
            let graph = &gr.graph;
            let base = type_color(graph.flags);
            let pos = |k: usize| self.position(NodeRef { graph: g, node: k });
            for (k, node) in graph.nodes.iter().enumerate() {
                let Some(p) = pos(k) else { continue };
                let selected = self.selected == Some(NodeRef { graph: g, node: k });
                let color = if selected {
                    SELECTED
                } else if node.flags & (node_flags::DISABLED | node_flags::BLOCKED) != 0 {
                    DISABLED
                } else {
                    base
                };
                // A cross of the node's radius and a post.
                let r = node.radius.clamp(0.1, 2.0);
                let c = lift(p);
                for (dx, dz) in [(r, 0.0), (0.0, r)] {
                    out.push(LineVertex::new([c[0] - dx, c[1], c[2] - dz], color));
                    out.push(LineVertex::new([c[0] + dx, c[1], c[2] + dz], color));
                }
                out.push(LineVertex::new(p, color));
                out.push(LineVertex::new([p[0], p[1] + 1.0, p[2]], color));
                for l in &node.links {
                    let Some(q) = pos(l.to as usize) else {
                        continue;
                    };
                    let color = if selected {
                        SELECTED
                    } else if l.flags
                        & (link_flags::MUST_VAULT
                            | link_flags::MUST_LADDER_CLIMB
                            | link_flags::FALL)
                        != 0
                    {
                        SPECIAL
                    } else if l.flags & link_flags::CROSSWALK != 0 {
                        CROSSWALK
                    } else {
                        base
                    };
                    let (a, b) = (lift(p), lift(q));
                    out.push(LineVertex::new(a, color));
                    out.push(LineVertex::new(b, color));
                    // One-way links get an arrow head at the target.
                    let back = graph.nodes[l.to as usize]
                        .links
                        .iter()
                        .any(|x| x.to as usize == k);
                    if !back {
                        let d = [b[0] - a[0], b[2] - a[2]];
                        let len = (d[0] * d[0] + d[1] * d[1]).sqrt().max(1e-3);
                        let (ux, uz) = (d[0] / len, d[1] / len);
                        let s = 0.6f32.min(len * 0.3);
                        for side in [-1.0f32, 1.0] {
                            out.push(LineVertex::new(b, color));
                            out.push(LineVertex::new(
                                [
                                    b[0] - ux * s - uz * s * 0.5 * side,
                                    b[1],
                                    b[2] - uz * s + ux * s * 0.5 * side,
                                ],
                                color,
                            ));
                        }
                    }
                }
            }
        }
        out
    }

    /// The shown node nearest to `pos` on the screen, within `max` points.
    pub fn pick(
        &self,
        to_screen: &dyn Fn([f32; 3]) -> Option<[f32; 2]>,
        pos: [f32; 2],
        max: f32,
    ) -> Option<NodeRef> {
        if !self.visible || !self.edit {
            return None;
        }
        let mut best: Option<(f32, NodeRef)> = None;
        for (g, gr) in self.graphs.iter().enumerate() {
            if !self.shown(g) {
                continue;
            }
            for (k, node) in gr.graph.nodes.iter().enumerate() {
                let p = node.position;
                let Some(s) = to_screen([p[0], p[1] + LIFT, p[2]]) else {
                    continue;
                };
                let d = ((s[0] - pos[0]).powi(2) + (s[1] - pos[1]).powi(2)).sqrt();
                if d <= max && best.is_none_or(|(b, _)| d < b) {
                    best = Some((d, NodeRef { graph: g, node: k }));
                }
            }
        }
        best.map(|(_, r)| r)
    }

    /// An edit of graph `g` as an action for the app.
    fn edit(
        &self,
        g: usize,
        label: String,
        f: impl FnOnce(&mut Graph) -> doctor_toolkit::Result<Option<Vec<Option<u16>>>>,
    ) -> Result<LevelAction, String> {
        let mut graph = self.graphs[g].clone();
        let renumbered = f(&mut graph.graph).map_err(|e| e.to_string())?;
        Ok(LevelAction::Graph {
            graph: Box::new(graph),
            renumbered,
            label: format!("{label} ({})", self.names[g]),
        })
    }

    /// Ends a gizmo drag of a node.
    pub fn moved(&mut self, r: NodeRef, to: [f32; 3]) -> Result<LevelAction, String> {
        self.preview = None;
        self.edit(r.graph, trf("navgraph.moved", &[("node", &r.node)]), |g| {
            g.move_node(r.node, to).map(|()| None)
        })
    }

    /// A link from the selected node to `to` (Shift + click).
    pub fn link_to(&self, to: NodeRef) -> Option<Result<LevelAction, String>> {
        let from = self.selected?;
        if from.graph != to.graph || from == to {
            return Some(Err(tr("navgraph.link_same_graph").to_owned()));
        }
        let two_way = self.two_way;
        Some(self.edit(
            from.graph,
            trf("navgraph.linked", &[("from", &from.node), ("to", &to.node)]),
            |g| {
                g.add_link(from.node, to.node, 0)?;
                if two_way
                    && !g.nodes[to.node]
                        .links
                        .iter()
                        .any(|l| l.to as usize == from.node)
                {
                    g.add_link(to.node, from.node, 0)?;
                }
                Ok(None)
            },
        ))
    }

    /// Adds a node at `at` linked with the nearest nodes of the shown
    /// graph nearest to it ("connect a new object to the graph").
    pub fn attach(&self, at: [f32; 3]) -> Result<LevelAction, String> {
        let mut best: Option<(f32, usize)> = None;
        for (g, gr) in self.graphs.iter().enumerate() {
            if !self.shown(g) {
                continue;
            }
            if let Some(k) = gr.graph.nearest_node(at) {
                let d = doctor_toolkit::resource::graph::distance(gr.graph.nodes[k].position, at);
                if best.is_none_or(|(b, _)| d < b) {
                    best = Some((d, g));
                }
            }
        }
        let g = match (self.selected, best) {
            (Some(r), _) => r.graph,
            (None, Some((_, g))) => g,
            (None, None) => return Err(tr("navgraph.no_graph").to_owned()),
        };
        let how = self.how;
        self.edit(g, tr("navgraph.attached").to_owned(), |graph| {
            graph.attach_point(at, 0.5, how).map(|_| None)
        })
    }

    /// The panel in the browser: switches, layer, the selected node.
    pub fn panel(
        &mut self,
        ui: &mut egui::Ui,
        object_at: Option<[f32; 3]>,
        camera_target: [f32; 3],
    ) -> Option<Result<LevelAction, String>> {
        let mut action = None;
        let before = (self.visible, self.layer, self.selected);
        ui.horizontal_wrapped(|ui| {
            ui.checkbox(&mut self.visible, tr("navgraph.show"))
                .on_hover_text(tr("navgraph.show_hint"));
            ui.add_enabled_ui(self.visible, |ui| {
                ui.checkbox(&mut self.edit, tr("navgraph.pick"))
                    .on_hover_text(tr("navgraph.pick_hint"));
            });
        });
        if !self.visible {
            if before != (self.visible, self.layer, self.selected) {
                self.generation += 1;
            }
            return None;
        }
        ui.horizontal_wrapped(|ui| {
            ui.label(tr("navgraph.layer"));
            ui.selectable_value(&mut self.layer, 0, tr("navgraph.all"));
            for (bit, name) in graph_flags::NAMES {
                ui.selectable_value(&mut self.layer, bit, name);
            }
        });
        let nodes: usize = self.graphs.iter().map(|g| g.graph.nodes.len()).sum();
        ui.label(
            RichText::new(trf(
                "navgraph.loaded",
                &[("graphs", &self.graphs.len()), ("nodes", &nodes)],
            ))
            .weak(),
        );
        for f in self.failed.iter().take(3) {
            ui.colored_label(ui.visuals().warn_fg_color, f);
        }
        ui.horizontal_wrapped(|ui| {
            ui.checkbox(&mut self.two_way, tr("navgraph.two_way"))
                .on_hover_text(tr("navgraph.two_way_hint"));
            ui.add(
                egui::DragValue::new(&mut self.how.max_links)
                    .range(1..=8)
                    .prefix(tr("navgraph.max_links")),
            );
            ui.add(
                egui::DragValue::new(&mut self.how.max_distance)
                    .range(1.0..=100.0)
                    .speed(0.5)
                    .prefix(tr("navgraph.max_distance")),
            );
        });
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(
                    object_at.is_some(),
                    egui::Button::new(tr("navgraph.attach_object")),
                )
                .on_hover_text(tr("navgraph.attach_object_hint"))
                .clicked()
                && let Some(at) = object_at
            {
                action = Some(self.attach(at));
            }
            if ui
                .button(tr("navgraph.attach_here"))
                .on_hover_text(tr("navgraph.attach_here_hint"))
                .clicked()
            {
                action = Some(self.attach(camera_target));
            }
        });
        if let Some(r) = self.selected
            && let Some(a) = self.node_editor(ui, r)
        {
            action = Some(a);
        } else if self.selected.is_none() {
            ui.label(RichText::new(tr("navgraph.select_hint")).weak());
        }
        if before != (self.visible, self.layer, self.selected) {
            self.generation += 1;
        }
        action
    }

    fn node_editor(
        &mut self,
        ui: &mut egui::Ui,
        r: NodeRef,
    ) -> Option<Result<LevelAction, String>> {
        let (node, title, stats) = {
            let graph = &self.graphs.get(r.graph)?.graph;
            let title = trf(
                "navgraph.node_title",
                &[
                    ("node", &r.node),
                    ("graph", &self.names[r.graph]),
                    ("type", &flag_names(graph.flags, &graph_flags::NAMES)),
                ],
            );
            let stats = trf(
                "navgraph.graph_stats",
                &[
                    ("nodes", &graph.nodes.len()),
                    ("links", &graph.link_count()),
                    ("neighbours", &graph.neighbour_link_count()),
                ],
            );
            (graph.nodes.get(r.node)?.clone(), title, stats)
        };
        let mut action = None;
        ui.separator();
        ui.label(RichText::new(title).strong());
        ui.label(RichText::new(stats).weak());
        let mut pos = self.position(r).unwrap_or(node.position);
        let mut commit = false;
        ui.horizontal(|ui| {
            for (k, axis) in ["X", "Y", "Z"].into_iter().enumerate() {
                let resp = ui.add(
                    egui::DragValue::new(&mut pos[k])
                        .speed(0.05)
                        .prefix(format!("{axis} ")),
                );
                if resp.changed() {
                    self.preview = Some((r, pos));
                    self.generation += 1;
                }
                commit |= resp.drag_stopped() || resp.lost_focus();
            }
        });
        if commit && pos != node.position {
            action = Some(self.moved(r, pos));
        }
        let mut disabled = node.flags & node_flags::DISABLED != 0;
        let mut radius = node.radius;
        ui.horizontal(|ui| {
            if ui
                .checkbox(&mut disabled, tr("navgraph.disabled"))
                .on_hover_text(tr("navgraph.disabled_hint"))
                .changed()
            {
                let flags = if disabled {
                    node.flags | node_flags::DISABLED
                } else {
                    node.flags & !node_flags::DISABLED
                };
                action = Some(
                    self.edit(r.graph, tr("navgraph.node_flags").to_owned(), |g| {
                        g.set_node_flags(r.node, flags).map(|()| None)
                    }),
                );
            }
            let resp = ui.add(
                egui::DragValue::new(&mut radius)
                    .range(0.0..=10.0)
                    .speed(0.01)
                    .prefix(tr("navgraph.radius")),
            );
            if (resp.drag_stopped() || resp.lost_focus()) && radius != node.radius {
                action = Some(
                    self.edit(r.graph, tr("navgraph.radius_set").to_owned(), |g| {
                        g.set_radius(r.node, radius).map(|()| None)
                    }),
                );
            }
        });
        ui.label(tr("navgraph.links"));
        for l in &node.links {
            ui.horizontal_wrapped(|ui| {
                if ui
                    .link(format!("→ {}", l.to))
                    .on_hover_text(tr("navgraph.goto_node"))
                    .clicked()
                {
                    self.selected = Some(NodeRef {
                        graph: r.graph,
                        node: l.to as usize,
                    });
                }
                ui.label(RichText::new(format!("{:.2} m", l.length)).weak());
                for (bit, name) in link_flags::NAMES {
                    let mut on = l.flags & bit != 0;
                    if ui.checkbox(&mut on, name).changed() {
                        let flags = l.flags ^ bit;
                        let to = l.to as usize;
                        action = Some(self.edit(
                            r.graph,
                            tr("navgraph.link_flags").to_owned(),
                            |g| g.set_link_flags(r.node, to, flags).map(|()| None),
                        ));
                    }
                }
                if ui
                    .small_button("✕")
                    .on_hover_text(tr("navgraph.unlink"))
                    .clicked()
                {
                    let to = l.to as usize;
                    action = Some(self.edit(
                        r.graph,
                        trf("navgraph.unlinked", &[("from", &r.node), ("to", &to)]),
                        |g| g.remove_link(r.node, to).map(|()| None),
                    ));
                }
            });
        }
        if !node.neighbour_links.is_empty() {
            ui.label(
                RichText::new(trf(
                    "navgraph.neighbour_links",
                    &[("count", &node.neighbour_links.len())],
                ))
                .weak(),
            );
        }
        ui.label(RichText::new(tr("navgraph.link_hint")).weak());
        ui.horizontal(|ui| {
            let text = if self.confirm_remove {
                tr("navgraph.remove_confirm")
            } else {
                tr("navgraph.remove")
            };
            if ui.button(text).clicked() {
                if self.confirm_remove {
                    self.confirm_remove = false;
                    action = Some(self.edit(
                        r.graph,
                        trf("navgraph.removed", &[("node", &r.node)]),
                        |g| g.remove_node(r.node).map(Some),
                    ));
                    self.selected = None;
                } else {
                    self.confirm_remove = true;
                }
            }
            if ui.button(tr("navgraph.deselect")).clicked() {
                self.selected = None;
                self.confirm_remove = false;
            }
        });
        action
    }

    /// Selects a node (or nothing).
    pub fn select(&mut self, r: Option<NodeRef>) {
        if self.selected != r {
            self.selected = r;
            self.confirm_remove = false;
            self.generation += 1;
        }
    }
}

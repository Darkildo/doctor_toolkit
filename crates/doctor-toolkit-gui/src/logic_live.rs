//! Messages as they go, on the logic graph (stage 35): the "Live" layer of
//! a logic tab.
//!
//! Fed with [`TraceEvent`]s — from the running game (the journal of the
//! live link) or from any other source of the same form (a run of the logic
//! without the game) — it lights the receivers, the likely senders and the
//! wires a message went along, keeps counters per node and wire, a feed of
//! the latest messages, and shows where the game stands at a breakpoint.
//! Events only bump totals ([`Activity`]); the glow is drawn from the time
//! each node or wire was last hit, so a busy game costs the same to draw as
//! a quiet one.
//!
//! **Stop-frame.** "Freeze" holds the picture and the feed while events keep
//! coming (they are kept and applied on "Go on") — a stop on the editor's
//! side when the game cannot or should not stop.

use std::collections::{HashMap, HashSet, VecDeque};
use std::time::{Duration, Instant};

use doctor_toolkit::live::proto::{BreakOn, BreakRule, Control, Stop, StopReason};
use doctor_toolkit::logic_graph::trace::{Activity, Hits, TraceEvent, TraceOrigin};
use doctor_toolkit::logic_graph::{LogicGraph, PinKind};
use doctor_toolkit_mods::i18n::{tr, trf};
use egui::{Color32, Pos2, RichText, Stroke};

use crate::node_graph::{Graph, GraphView, NodeId};

/// How long a hit glows.
const GLOW: Duration = Duration::from_millis(1600);
/// Events kept while frozen.
const FROZEN_KEEP: usize = 20_000;

const RECEIVED: Color32 = Color32::from_rgb(90, 230, 120);
const SENT: Color32 = Color32::from_rgb(255, 190, 70);
const WIRE: Color32 = Color32::from_rgb(120, 220, 255);
const STOPPED: Color32 = Color32::from_rgb(255, 80, 80);
const BREAK: Color32 = Color32::from_rgb(220, 40, 40);

/// Where the game stands, for people.
pub fn place_name(place: doctor_toolkit::live::proto::StopPlace) -> &'static str {
    use doctor_toolkit::live::proto::StopPlace;
    match place {
        StopPlace::Frame => tr("debug.place_frame"),
        StopPlace::Bus => tr("debug.place_bus"),
        StopPlace::Print => tr("debug.place_print"),
        StopPlace::Lua => tr("debug.place_lua"),
    }
}

/// What the layer asks the live link to do.
#[derive(Debug, Clone, PartialEq)]
pub enum LiveRequest {
    /// Add a breakpoint.
    Break(BreakRule),
    /// Remove the breakpoints of this message (and receiver).
    Unbreak(BreakOn),
    Control(Control),
}

/// What the app tells the layer every frame about the link.
#[derive(Debug, Clone, Default)]
pub struct LinkView {
    /// The link is up.
    pub connected: bool,
    /// The module has breakpoints (protocol 3).
    pub can_break: bool,
    pub stop: Option<Stop>,
    pub rules: Vec<BreakRule>,
    /// Events lost by the journal since the link came up.
    pub lost: u64,
}

/// The layer of one logic tab.
#[derive(Default)]
pub struct LiveOverlay {
    /// Follow the game: light what passes.
    pub on: bool,
    /// Keep only events that touch the selected nodes.
    only_selection: bool,
    /// Show in the feed also messages that did not touch the graph.
    show_outside: bool,
    /// Stop-frame: the picture holds, events wait.
    frozen: bool,
    waiting: VecDeque<TraceEvent>,
    activity: Option<Activity>,
    /// Size of the graph the totals belong to (a reload resets them).
    graph_size: (usize, usize),
    /// When each node and wire was last lit, and how.
    lit_nodes: HashMap<usize, (Instant, bool)>,
    lit_wires: HashMap<usize, Instant>,
    /// The feed row under the pointer or clicked: its hits drawn strongly.
    focus: Option<Hits>,
    /// Ask the app to select these nodes.
    select: Option<Vec<usize>>,
    requests: Vec<LiveRequest>,
    link: LinkView,
}

impl LiveOverlay {
    /// The graph was (re)loaded.
    pub fn graph_changed(&mut self, graph: &LogicGraph) {
        let size = (graph.nodes.len(), graph.wires.len());
        if self.activity.is_none() || size != self.graph_size {
            self.activity = Some(Activity::new(graph));
            self.graph_size = size;
            self.lit_nodes.clear();
            self.lit_wires.clear();
            self.focus = None;
        } else if let Some(a) = &mut self.activity {
            // Same shape (positions or names changed): keep the totals.
            a.map = doctor_toolkit::logic_graph::trace::TraceMap::new(graph);
        }
    }

    /// The state of the link this frame.
    pub fn set_link(&mut self, link: LinkView) {
        self.link = link;
    }

    /// Requests for the link since the last call.
    pub fn take_requests(&mut self) -> Vec<LiveRequest> {
        std::mem::take(&mut self.requests)
    }

    /// Nodes to select (a feed row was clicked).
    pub fn take_selection(&mut self) -> Option<Vec<usize>> {
        self.select.take()
    }

    /// New events. `selected` are the selected nodes of the tab.
    pub fn feed(&mut self, graph: &LogicGraph, events: &[TraceEvent], selected: &HashSet<usize>) {
        if !self.on || events.is_empty() {
            return;
        }
        if self.frozen {
            self.waiting.extend(events.iter().cloned());
            while self.waiting.len() > FROZEN_KEEP {
                self.waiting.pop_front();
            }
            return;
        }
        self.apply(graph, events.iter().cloned(), selected);
    }

    fn apply(
        &mut self,
        graph: &LogicGraph,
        events: impl Iterator<Item = TraceEvent>,
        selected: &HashSet<usize>,
    ) {
        if self.activity.is_none() {
            self.graph_changed(graph);
        }
        let Some(activity) = self.activity.as_mut() else {
            return;
        };
        let only_selection = self.only_selection && !selected.is_empty();
        let keep = |_: &TraceEvent, h: &Hits| {
            !only_selection
                || h.received
                    .iter()
                    .chain(&h.sent)
                    .any(|n| selected.contains(n))
        };
        let now = Instant::now();
        for e in events {
            if let Some(h) = activity.add(graph, e, &keep) {
                for &n in &h.received {
                    self.lit_nodes.insert(n, (now, true));
                }
                for &n in &h.sent {
                    self.lit_nodes.entry(n).or_insert((now, false)).0 = now;
                }
                for &w in &h.wires {
                    self.lit_wires.insert(w, now);
                }
            }
        }
    }

    /// Whether anything still glows (the tab repaints meanwhile).
    pub fn animating(&self) -> bool {
        let now = Instant::now();
        self.lit_nodes
            .values()
            .any(|(t, _)| now.duration_since(*t) < GLOW)
            || self
                .lit_wires
                .values()
                .any(|t| now.duration_since(*t) < GLOW)
    }

    fn glow(t: Instant) -> f32 {
        let age = Instant::now().duration_since(t).as_secs_f32() / GLOW.as_secs_f32();
        (1.0 - age).clamp(0.0, 1.0)
    }

    /// The line of the toolbar.
    pub fn toolbar(&mut self, ui: &mut egui::Ui, graph: Option<&LogicGraph>) {
        ui.separator();
        let mut on = self.on;
        if ui
            .toggle_value(&mut on, tr("debug.live"))
            .on_hover_text(tr("debug.live_hint"))
            .changed()
        {
            self.on = on;
            if on && let Some(g) = graph {
                self.graph_changed(g);
            }
        }
        if !self.on {
            return;
        }
        if !self.link.connected {
            ui.label(RichText::new(tr("debug.no_link")).weak());
        }
        let mut frozen = self.frozen;
        if ui
            .toggle_value(&mut frozen, tr("debug.freeze"))
            .on_hover_text(tr("debug.freeze_hint"))
            .changed()
        {
            self.frozen = frozen;
            if !frozen && let Some(g) = graph {
                let waiting: Vec<TraceEvent> = self.waiting.drain(..).collect();
                self.apply(g, waiting.into_iter(), &HashSet::new());
            }
        }
        if self.frozen && !self.waiting.is_empty() {
            ui.label(
                RichText::new(trf("debug.waiting", &[("count", &self.waiting.len())]))
                    .color(Color32::YELLOW),
            );
        }
        if self.link.can_break {
            if self.link.stop.is_some() {
                if ui.button(tr("debug.continue")).clicked() {
                    self.requests.push(LiveRequest::Control(Control::Continue));
                }
                if ui
                    .button(tr("debug.step"))
                    .on_hover_text(tr("debug.step_hint"))
                    .clicked()
                {
                    self.requests.push(LiveRequest::Control(Control::Step));
                }
            } else if ui
                .button(tr("debug.pause"))
                .on_hover_text(tr("debug.pause_hint"))
                .clicked()
            {
                self.requests.push(LiveRequest::Control(Control::Pause));
            }
        }
    }

    /// Message name: the graph's, else the hash.
    fn message_name(graph: &LogicGraph, e: &TraceEvent) -> String {
        e.name
            .clone()
            .or_else(|| graph.messages.get(&e.hash).map(|m| m.name.clone()))
            .or_else(|| {
                doctor_toolkit::logic_graph::LogicSchema::gf2()
                    .engine_message(e.hash)
                    .map(str::to_owned)
            })
            .unwrap_or_else(|| format!("{:#010x}", e.hash))
    }

    fn node_label(graph: &LogicGraph, n: usize) -> String {
        graph.nodes.get(n).map_or_else(|| "?".into(), |n| n.title())
    }

    /// The "Live" part of the side panel: totals, the stop, the feed,
    /// breakpoints of the selected node.
    pub fn side_panel(&mut self, ui: &mut egui::Ui, graph: &LogicGraph, selected: Option<usize>) {
        if !self.on {
            return;
        }
        egui::CollapsingHeader::new(RichText::new(tr("debug.live_title")).strong())
            .id_salt("logic-live")
            .default_open(true)
            .show(ui, |ui| self.side_inner(ui, graph, selected));
        ui.separator();
    }

    fn side_inner(&mut self, ui: &mut egui::Ui, graph: &LogicGraph, selected: Option<usize>) {
        if let Some(stop) = self.link.stop.clone() {
            let text = match &stop.reason {
                StopReason::Pause => tr("debug.stop_pause").to_owned(),
                StopReason::Step => tr("debug.stop_step").to_owned(),
                StopReason::Message {
                    hash, receivers, ..
                } => {
                    let e = TraceEvent {
                        seq: stop.seq,
                        time_ms: u64::from(stop.sim_time_ms),
                        frame: Some(stop.frame),
                        hash: *hash,
                        name: None,
                        sender: None,
                        receivers: receivers
                            .iter()
                            .map(doctor_toolkit::live::proto::guid_hex)
                            .collect(),
                        listeners: receivers.len() as u32,
                        count: 1,
                        origin: TraceOrigin::Game,
                    };
                    trf(
                        "debug.stop_message",
                        &[("message", &Self::message_name(graph, &e))],
                    )
                }
                StopReason::Output { text, .. } => trf("debug.stop_print", &[("text", text)]),
                StopReason::LuaError { message } => trf("debug.stop_lua", &[("message", message)]),
            };
            ui.colored_label(STOPPED, RichText::new(text).strong());
            ui.label(
                RichText::new(trf(
                    "debug.stop_where",
                    &[
                        ("place", &crate::logic_live::place_name(stop.place)),
                        ("frame", &stop.frame),
                        (
                            "seconds",
                            &format!("{:.0}", f64::from(stop.resume_in_ms) / 1000.0),
                        ),
                    ],
                ))
                .small(),
            );
        }
        if let Some(a) = &self.activity {
            ui.label(
                RichText::new(trf(
                    "debug.totals",
                    &[
                        ("seen", &a.seen),
                        ("matched", &a.matched),
                        ("lost", &self.link.lost),
                    ],
                ))
                .small(),
            );
        }
        ui.horizontal_wrapped(|ui| {
            ui.checkbox(&mut self.only_selection, tr("debug.only_selection"))
                .on_hover_text(tr("debug.only_selection_hint"));
            ui.checkbox(&mut self.show_outside, tr("debug.show_outside"))
                .on_hover_text(tr("debug.show_outside_hint"));
            if ui.button(tr("log.clear")).clicked()
                && let Some(a) = &mut self.activity
            {
                a.clear();
                self.lit_nodes.clear();
                self.lit_wires.clear();
                self.focus = None;
            }
            if ui
                .button(tr("debug.save_trace"))
                .on_hover_text(tr("debug.save_trace_hint"))
                .clicked()
                && let Some(a) = &self.activity
            {
                let events: Vec<TraceEvent> = a
                    .feed
                    .iter()
                    .map(|(e, _)| {
                        let mut e = e.clone();
                        if e.name.is_none() {
                            e.name = Some(Self::message_name(graph, &e));
                        }
                        e
                    })
                    .collect();
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter("JSON lines", &["jsonl"])
                    .set_file_name("trace.jsonl")
                    .save_file()
                {
                    let mut out = Vec::new();
                    let _ = doctor_toolkit::logic_graph::trace::write_jsonl(&events, &mut out);
                    let _ = std::fs::write(path, out);
                }
            }
        });
        // Breakpoints on what the selected node hears.
        if self.link.can_break
            && let Some(n) = selected
            && let Some(node) = graph.nodes.get(n)
            && let Some(guid) = node
                .guid
                .as_deref()
                .and_then(doctor_toolkit::live::proto::parse_guid)
        {
            let heard: Vec<(String, u32)> = node
                .pins
                .iter()
                .filter(|p| p.kind == PinKind::In)
                .filter_map(|p| Some((p.message.clone()?, p.hash?)))
                .collect();
            if !heard.is_empty() {
                ui.label(RichText::new(tr("debug.break_on")).small());
                ui.horizontal_wrapped(|ui| {
                    for (name, hash) in heard {
                        let on = BreakOn::Message {
                            hash,
                            receiver: Some(guid),
                        };
                        let set = self.link.rules.iter().any(|r| r.on == on);
                        let mut checked = set;
                        if ui
                            .toggle_value(&mut checked, name.clone())
                            .on_hover_text(tr("debug.break_on_hint"))
                            .changed()
                        {
                            self.requests.push(if checked {
                                LiveRequest::Break(BreakRule { on, once: false })
                            } else {
                                LiveRequest::Unbreak(on)
                            });
                        }
                    }
                });
            }
        }
        // The feed, newest first.
        let Some(a) = &self.activity else { return };
        let mut hovered: Option<Hits> = None;
        let mut clicked: Option<Vec<usize>> = None;
        let rows: Vec<(String, Hits, bool)> = a
            .feed
            .iter()
            .rev()
            .filter(|(_, h)| self.show_outside || !h.is_empty())
            .take(200)
            .map(|(e, h)| {
                let to: Vec<String> = h
                    .received
                    .iter()
                    .take(3)
                    .map(|&n| Self::node_label(graph, n))
                    .collect();
                let from: Vec<String> = h
                    .sent
                    .iter()
                    .take(2)
                    .map(|&n| Self::node_label(graph, n))
                    .collect();
                let mut text = format!(
                    "{:>7.1}s {}",
                    e.time_ms as f64 / 1000.0,
                    Self::message_name(graph, e)
                );
                if e.count > 1 {
                    text += &format!(" ×{}", e.count);
                }
                if !from.is_empty() {
                    text += &format!("  {} ->", from.join(", "));
                }
                if !to.is_empty() {
                    text += &format!(" {}", to.join(", "));
                    if h.received.len() > 3 {
                        text += &format!(" +{}", h.received.len() - 3);
                    }
                } else if e.listeners > 0 {
                    text += &format!(" -> {} listener(s)", e.listeners);
                }
                if e.origin == TraceOrigin::Link {
                    text += " (link)";
                }
                (text, h.clone(), h.is_empty())
            })
            .collect();
        if rows.is_empty() {
            ui.label(RichText::new(tr("debug.feed_empty")).weak().small());
            return;
        }
        egui::ScrollArea::vertical()
            .id_salt("logic-live-feed")
            .max_height(260.0)
            .show(ui, |ui| {
                for (text, hits, outside) in &rows {
                    let rich = RichText::new(text).monospace().small();
                    let rich = if *outside { rich.weak() } else { rich };
                    let r = ui.add(
                        egui::Label::new(rich)
                            .truncate()
                            .sense(egui::Sense::click()),
                    );
                    if r.hovered() {
                        hovered = Some(hits.clone());
                    }
                    if r.clicked() {
                        clicked = Some(hits.received.iter().chain(&hits.sent).copied().collect());
                    }
                }
            });
        self.focus = hovered;
        if let Some(c) = clicked.filter(|c| !c.is_empty()) {
            self.select = Some(c);
        }
    }

    /// Draws the glow over the graph widget: `canvas` is the rect of its
    /// response, `wire_of` maps widget wires to graph wires.
    pub fn paint(
        &self,
        ui: &egui::Ui,
        canvas: egui::Rect,
        view: &GraphView,
        shown: &Graph,
        wire_of: &[Option<usize>],
        graph: &LogicGraph,
    ) {
        if !self.on {
            return;
        }
        let painter = ui.painter_at(canvas);
        let zoom = view.zoom;
        let node_rect = |n: usize| {
            shown.node(NodeId(n as u64)).map(|node| {
                let r = node.rect();
                egui::Rect::from_min_max(
                    view.graph_to_screen(canvas, r.min),
                    view.graph_to_screen(canvas, r.max),
                )
            })
        };
        let focus_wires: HashSet<usize> = self
            .focus
            .as_ref()
            .map(|h| h.wires.iter().copied().collect())
            .unwrap_or_default();
        // Wires.
        for (i, w) in shown.wires.iter().enumerate() {
            let Some(gw) = wire_of.get(i).copied().flatten() else {
                continue;
            };
            let glow = self.lit_wires.get(&gw).map_or(0.0, |t| Self::glow(*t));
            let focused = focus_wires.contains(&gw);
            if glow <= 0.0 && !focused {
                continue;
            }
            let Some(points) = view.wire_screen_points(shown, canvas, w.from, w.to) else {
                continue;
            };
            let alpha = if focused { 1.0 } else { glow };
            let width = (2.0 + 4.0 * alpha) * zoom.clamp(0.5, 1.5);
            painter.add(egui::Shape::line(
                points.clone(),
                Stroke::new(width, WIRE.gamma_multiply(alpha)),
            ));
            // A dot running along the wire while it glows.
            if glow > 0.0 && points.len() > 1 {
                let t = 1.0 - glow;
                let k = ((points.len() - 1) as f32 * t).round() as usize;
                painter.circle_filled(
                    points[k.min(points.len() - 1)],
                    4.0 * zoom.clamp(0.6, 1.4),
                    Color32::WHITE.gamma_multiply(glow),
                );
            }
        }
        // Nodes.
        let focus_nodes: HashMap<usize, bool> = self
            .focus
            .as_ref()
            .map(|h| {
                h.received
                    .iter()
                    .map(|&n| (n, true))
                    .chain(h.sent.iter().map(|&n| (n, false)))
                    .collect()
            })
            .unwrap_or_default();
        let mut nodes: HashMap<usize, (f32, bool)> = self
            .lit_nodes
            .iter()
            .map(|(&n, &(t, received))| (n, (Self::glow(t), received)))
            .filter(|(_, (g, _))| *g > 0.0)
            .collect();
        for (&n, &received) in &focus_nodes {
            nodes.insert(n, (1.0, received));
        }
        for (n, (glow, received)) in nodes {
            let Some(r) = node_rect(n) else { continue };
            let color = if received { RECEIVED } else { SENT };
            painter.rect_stroke(
                r.expand(3.0 * zoom.max(0.5)),
                6.0 * zoom,
                Stroke::new(2.0 + 3.0 * glow, color.gamma_multiply(glow.max(0.3))),
                egui::StrokeKind::Outside,
            );
        }
        // Counters.
        if let Some(a) = &self.activity
            && zoom > 0.35
        {
            let counted: HashSet<usize> = a
                .nodes_received
                .keys()
                .chain(a.nodes_sent.keys())
                .copied()
                .collect();
            for n in counted {
                let Some(r) = node_rect(n) else { continue };
                let got = a.nodes_received.get(&n).map_or(0, |s| s.count);
                let sent = a.nodes_sent.get(&n).map_or(0, |s| s.count);
                let text = match (got, sent) {
                    (0, s) => format!("out {s}"),
                    (g, 0) => format!("in {g}"),
                    (g, s) => format!("in {g} out {s}"),
                };
                let at = Pos2::new(r.max.x, r.min.y - 2.0);
                painter.text(
                    at,
                    egui::Align2::RIGHT_BOTTOM,
                    text,
                    egui::FontId::monospace(11.0),
                    Color32::from_gray(230),
                );
            }
        }
        // Breakpoints: a red dot on the receivers and on the wires of their
        // messages.
        for rule in &self.link.rules {
            let BreakOn::Message { hash, receiver } = &rule.on else {
                continue;
            };
            let receiver_nodes: Vec<usize> = match receiver {
                Some(g) => self
                    .activity
                    .as_ref()
                    .map(|a| {
                        a.map
                            .nodes_of(&doctor_toolkit::live::proto::guid_hex(g))
                            .to_vec()
                    })
                    .unwrap_or_default(),
                None => graph
                    .wires
                    .iter()
                    .filter(|w| w.message == Some(*hash))
                    .map(|w| w.to.node)
                    .collect(),
            };
            for n in receiver_nodes {
                if let Some(r) = node_rect(n) {
                    painter.circle(
                        r.min + egui::vec2(0.0, 0.0),
                        6.0,
                        BREAK,
                        Stroke::new(1.5, Color32::WHITE),
                    );
                }
            }
        }
        // Where the game stands.
        if let Some(Stop {
            reason: StopReason::Message { receivers, .. },
            ..
        }) = &self.link.stop
            && let Some(a) = &self.activity
        {
            for g in receivers {
                for &n in a.map.nodes_of(&doctor_toolkit::live::proto::guid_hex(g)) {
                    if let Some(r) = node_rect(n) {
                        painter.rect_stroke(
                            r.expand(6.0),
                            8.0,
                            Stroke::new(4.0, STOPPED),
                            egui::StrokeKind::Outside,
                        );
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use doctor_toolkit::logic_graph::{Message, MessageState, NodeSource, PinRef, Wire, WireKind};

    fn graph() -> LogicGraph {
        let mut g = LogicGraph::default();
        for (i, guid) in ["aa", "bb"].iter().enumerate() {
            g.nodes.push(doctor_toolkit::logic_graph::Node {
                source: NodeSource::Object {
                    group: doctor_toolkit::level::Location {
                        stream: 0,
                        resource: 0,
                    },
                    object: i,
                },
                stream: String::new(),
                class: 0,
                class_name: "MessageRelay".into(),
                category: doctor_toolkit::logic_graph::Category::Relay,
                name: (*guid).into(),
                guid: Some(guid.repeat(16)),
                position: None,
                pins: Vec::new(),
            });
        }
        g.wires.push(Wire {
            from: PinRef { node: 0, pin: 0 },
            to: PinRef { node: 1, pin: 0 },
            kind: WireKind::Message,
            message: Some(7),
        });
        g.messages.insert(
            7,
            Message {
                hash: 7,
                name: "doctor_ping".into(),
                senders: Vec::new(),
                listeners: Vec::new(),
                state: MessageState::Connected,
                engine: false,
                hub: None,
                elsewhere: None,
            },
        );
        g
    }

    fn event(seq: u64) -> TraceEvent {
        TraceEvent {
            seq,
            time_ms: seq,
            frame: None,
            hash: 7,
            name: None,
            sender: None,
            receivers: vec!["bb".repeat(16)],
            listeners: 1,
            count: 1,
            origin: TraceOrigin::Game,
        }
    }

    #[test]
    fn lights_counts_and_freezes() {
        let g = graph();
        let mut o = LiveOverlay::default();
        // Off: nothing.
        o.feed(&g, &[event(1)], &HashSet::new());
        assert!(o.activity.is_none());
        o.on = true;
        o.graph_changed(&g);
        let many: Vec<TraceEvent> = (0..5000).map(event).collect();
        o.feed(&g, &many, &HashSet::new());
        let a = o.activity.as_ref().unwrap();
        assert_eq!(a.wires[&0].count, 5000);
        assert_eq!(a.nodes_received[&1].count, 5000);
        assert_eq!(a.nodes_sent[&0].count, 5000);
        assert!(o.animating());
        assert!(o.lit_nodes[&1].1);
        assert!(!o.lit_nodes[&0].1);
        // Frozen: events wait and come in on going on.
        o.frozen = true;
        o.feed(&g, &[event(9000)], &HashSet::new());
        assert_eq!(o.activity.as_ref().unwrap().seen, 5000);
        assert_eq!(o.waiting.len(), 1);
        o.frozen = false;
        let waiting: Vec<TraceEvent> = o.waiting.drain(..).collect();
        o.apply(&g, waiting.into_iter(), &HashSet::new());
        assert_eq!(o.activity.as_ref().unwrap().seen, 5001);
        // Only the selection: node 0 alone is selected and sent it — kept;
        // an event elsewhere is not.
        o.only_selection = true;
        let sel: HashSet<usize> = [0].into();
        let mut other = event(9001);
        other.hash = 99;
        other.receivers = vec!["cc".repeat(16)];
        o.feed(&g, &[event(9002), other], &sel);
        assert_eq!(o.activity.as_ref().unwrap().seen, 5002);
        // A reload of another shape starts again.
        let mut bigger = graph();
        bigger.wires.push(bigger.wires[0].clone());
        o.graph_changed(&bigger);
        assert_eq!(o.activity.as_ref().unwrap().seen, 0);
    }
}

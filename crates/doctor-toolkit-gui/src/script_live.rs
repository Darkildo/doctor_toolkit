//! The journal of a visual script in the running game (stage 35).
//!
//! A script compiled with "trace nodes" ([`ScriptGraph::trace_nodes`])
//! prints `dt:<script>:<node>;` from every node that runs; the game's
//! journal (live link) brings those lines, and Lua errors with their chunk
//! and line. Here they become "node N fired" and "node N failed": the nodes
//! glow on the graph, the journal lists them, and a breakpoint on a node is
//! a breakpoint on its trace text — the game stops in that node's `print`.

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use doctor_toolkit::live::proto::journal::EventBody;
use doctor_toolkit::live::proto::{BreakOn, BreakRule, Event, Stop, StopReason};
use doctor_toolkit::script::Compiled;
use doctor_toolkit::script::compile::{parse_trace, trace_text};
use doctor_toolkit::script::graph::ScriptGraph;
use doctor_toolkit_mods::i18n::{tr, trf};
use egui::{Color32, RichText, Stroke};

use crate::logic_live::{LinkView, LiveRequest};
use crate::node_graph::{Graph, GraphView, NodeId};

const GLOW: Duration = Duration::from_millis(1600);
const KEEP: usize = 2000;
const FIRED: Color32 = Color32::from_rgb(90, 230, 120);
const FAILED: Color32 = Color32::from_rgb(255, 80, 80);

/// One line of the journal.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub time_ms: u32,
    pub node: Option<u32>,
    pub text: String,
    pub error: bool,
}

/// The game's side of the script editor.
#[derive(Default)]
pub struct ScriptLive {
    pub entries: VecDeque<Entry>,
    /// Times each node fired and when last.
    pub fired: HashMap<u32, (u64, Option<Instant>)>,
    failed: HashMap<u32, Instant>,
    requests: Vec<LiveRequest>,
    link: LinkView,
}

impl ScriptLive {
    pub fn set_link(&mut self, link: LinkView) {
        self.link = link;
    }

    pub fn take_requests(&mut self) -> Vec<LiveRequest> {
        std::mem::take(&mut self.requests)
    }

    /// Events of the game's journal: prints of traced nodes of `graph` and
    /// Lua errors in its chunk.
    pub fn feed(&mut self, graph: &ScriptGraph, compiled: &Compiled, events: &[Event]) {
        let chunk = graph.chunk_name();
        let now = Instant::now();
        for e in events {
            let entry = match &e.body {
                EventBody::Output { text, .. } => match parse_trace(text) {
                    Some((script, node)) if script == graph.name => {
                        let f = self.fired.entry(node).or_default();
                        f.0 += 1;
                        f.1 = Some(now);
                        Entry {
                            time_ms: e.sim_time_ms,
                            node: Some(node),
                            text: String::new(),
                            error: false,
                        }
                    }
                    _ => continue,
                },
                EventBody::LuaError {
                    message,
                    chunk: c,
                    line,
                    traceback,
                    ..
                } => {
                    // The innermost level in this chunk: its line maps to
                    // the node that made it.
                    let at = if *c == chunk && *line > 0 {
                        Some(*line as u32)
                    } else {
                        traceback.iter().find_map(|t| {
                            t.strip_prefix(&chunk)?
                                .strip_prefix(':')?
                                .split(':')
                                .next()?
                                .parse()
                                .ok()
                        })
                    };
                    let Some(line) = at else { continue };
                    let node = compiled.node_at_line(line);
                    if let Some(n) = node {
                        self.failed.insert(n, now);
                    }
                    Entry {
                        time_ms: e.sim_time_ms,
                        node,
                        text: message.clone(),
                        error: true,
                    }
                }
                _ => continue,
            };
            self.entries.push_back(entry);
            while self.entries.len() > KEEP {
                self.entries.pop_front();
            }
        }
    }

    pub fn animating(&self) -> bool {
        let now = Instant::now();
        self.fired
            .values()
            .filter_map(|f| f.1)
            .chain(self.failed.values().copied())
            .any(|t| now.duration_since(t) < GLOW)
    }

    fn glow(t: Instant) -> f32 {
        (1.0 - Instant::now().duration_since(t).as_secs_f32() / GLOW.as_secs_f32()).clamp(0.0, 1.0)
    }

    /// The breakpoint of node `id` of `graph`.
    fn rule(graph: &ScriptGraph, id: u32) -> BreakOn {
        BreakOn::Output {
            text: trace_text(&graph.name, id),
        }
    }

    /// Where the game stands, when it is at a node of this script.
    fn stopped_at(&self, graph: &ScriptGraph) -> Option<u32> {
        match &self.link.stop {
            Some(Stop {
                reason: StopReason::Output { text, .. },
                ..
            }) => parse_trace(text)
                .filter(|(s, _)| *s == graph.name)
                .map(|(_, n)| n),
            _ => None,
        }
    }

    /// In the inspector of node `id`: how often it fired and its breakpoint.
    pub fn node_ui(&mut self, ui: &mut egui::Ui, graph: &ScriptGraph, id: u32) {
        if !self.link.connected {
            return;
        }
        ui.separator();
        let fired = self.fired.get(&id).map_or(0, |f| f.0);
        ui.label(trf("debug.node_fired", &[("count", &fired)]));
        if !graph.trace_nodes {
            ui.label(RichText::new(tr("debug.trace_off")).weak().small());
            return;
        }
        if self.link.can_break {
            let on = Self::rule(graph, id);
            let mut set = self.link.rules.iter().any(|r| r.on == on);
            if ui
                .checkbox(&mut set, tr("debug.node_break"))
                .on_hover_text(tr("debug.node_break_hint"))
                .changed()
            {
                self.requests.push(if set {
                    LiveRequest::Break(BreakRule { on, once: false })
                } else {
                    LiveRequest::Unbreak(on)
                });
            }
        }
    }

    /// The "Game" tab of the bottom panel. Returns a node to select.
    pub fn journal_ui(&mut self, ui: &mut egui::Ui, graph: &ScriptGraph) -> Option<u32> {
        let mut select = None;
        ui.horizontal_wrapped(|ui| {
            if !self.link.connected {
                ui.label(RichText::new(tr("debug.no_link")).weak());
            }
            if !graph.trace_nodes {
                ui.label(RichText::new(tr("debug.trace_off")).weak());
            }
            if let Some(n) = self.stopped_at(graph) {
                ui.colored_label(FAILED, trf("debug.script_stopped", &[("node", &n)]));
                if ui.button(tr("debug.continue")).clicked() {
                    self.requests.push(LiveRequest::Control(
                        doctor_toolkit::live::proto::Control::Continue,
                    ));
                }
                if ui.button(tr("debug.show_node")).clicked() {
                    select = Some(n);
                }
            }
            if ui.button(tr("log.clear")).clicked() {
                self.entries.clear();
                self.fired.clear();
                self.failed.clear();
            }
        });
        egui::ScrollArea::vertical()
            .auto_shrink(false)
            .stick_to_bottom(true)
            .show(ui, |ui| {
                if self.entries.is_empty() {
                    ui.label(RichText::new(tr("debug.script_journal_empty")).weak());
                }
                for e in &self.entries {
                    let title = e
                        .node
                        .and_then(|n| graph.node(n))
                        .map(|n| {
                            doctor_toolkit::script::library::get(&n.kind)
                                .map_or(n.kind.clone(), |d| d.title.to_owned())
                        })
                        .unwrap_or_default();
                    let text = match (e.node, e.error) {
                        (Some(n), false) => {
                            format!("{:>8.1}s  [{n}] {title}", f64::from(e.time_ms) / 1000.0)
                        }
                        (Some(n), true) => format!(
                            "{:>8.1}s  [{n}] {title}: {}",
                            f64::from(e.time_ms) / 1000.0,
                            e.text
                        ),
                        (None, _) => format!("{:>8.1}s  {}", f64::from(e.time_ms) / 1000.0, e.text),
                    };
                    let rich = RichText::new(text).monospace().small();
                    let rich = if e.error { rich.color(FAILED) } else { rich };
                    if ui
                        .add(egui::Label::new(rich).sense(egui::Sense::click()))
                        .clicked()
                    {
                        select = e.node;
                    }
                }
            });
        select
    }

    /// Glow of the nodes that fired or failed, the node where the game
    /// stands, and breakpoints.
    pub fn paint(
        &self,
        ui: &egui::Ui,
        canvas: egui::Rect,
        view: &GraphView,
        shown: &Graph,
        graph: &ScriptGraph,
    ) {
        let painter = ui.painter_at(canvas);
        let rect = |id: u32| {
            shown.node(NodeId(u64::from(id))).map(|n| {
                let r = n.rect();
                egui::Rect::from_min_max(
                    view.graph_to_screen(canvas, r.min),
                    view.graph_to_screen(canvas, r.max),
                )
            })
        };
        let z = view.zoom;
        for (&id, f) in &self.fired {
            if let (Some(t), Some(r)) = (f.1, rect(id)) {
                let g = Self::glow(t);
                if g > 0.0 {
                    painter.rect_stroke(
                        r.expand(3.0),
                        6.0 * z,
                        Stroke::new(2.0 + 3.0 * g, FIRED.gamma_multiply(g)),
                        egui::StrokeKind::Outside,
                    );
                }
            }
            if let Some(r) = rect(id).filter(|_| z > 0.35) {
                painter.text(
                    egui::pos2(r.max.x, r.min.y - 2.0),
                    egui::Align2::RIGHT_BOTTOM,
                    format!("×{}", f.0),
                    egui::FontId::monospace(11.0),
                    Color32::from_gray(230),
                );
            }
        }
        for (&id, &t) in &self.failed {
            if let Some(r) = rect(id) {
                let g = Self::glow(t).max(0.35);
                painter.rect_stroke(
                    r.expand(3.0),
                    6.0 * z,
                    Stroke::new(3.0, FAILED.gamma_multiply(g)),
                    egui::StrokeKind::Outside,
                );
            }
        }
        for rule in &self.link.rules {
            if let BreakOn::Output { text } = &rule.on
                && let Some((s, n)) = parse_trace(text)
                && s == graph.name
                && let Some(r) = rect(n)
            {
                painter.circle(r.min, 6.0, FAILED, Stroke::new(1.5, Color32::WHITE));
            }
        }
        if let Some(n) = self.stopped_at(graph)
            && let Some(r) = rect(n)
        {
            painter.rect_stroke(
                r.expand(6.0),
                8.0,
                Stroke::new(4.0, FAILED),
                egui::StrokeKind::Outside,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use doctor_toolkit::live::proto::journal::OutputSource;
    use doctor_toolkit::script::compile;
    use doctor_toolkit::script::graph::Node;

    #[test]
    fn traced_prints_and_errors_become_nodes() {
        let mut g = ScriptGraph::new("demo");
        let ev = g.add(Node::new(0, "event.message", [0.0; 2]).param("message", "iMsgPing"));
        let send =
            g.add(Node::new(0, "action.send_message", [0.0; 2]).input("message", "doctor_pong"));
        g.connect((ev, "then"), (send, "exec"));
        g.trace_nodes = true;
        let c = compile(&g);
        let line = c
            .source
            .lines()
            .position(|l| l.contains("SendMessage("))
            .unwrap() as i32
            + 1;
        let event = |seq, body| Event {
            seq,
            frame: 1,
            sim_time_ms: 1000,
            body,
        };
        let print = |text: &str| EventBody::Output {
            source: OutputSource::Print,
            text: text.into(),
            script: "demo.lua".into(),
            line: 3,
        };
        let mut live = ScriptLive::default();
        live.feed(
            &g,
            &c,
            &[
                event(1, print(&trace_text("demo", ev))),
                event(2, print(&trace_text("demo", send))),
                event(3, print(&trace_text("other", send))),
                event(4, print("hello")),
                event(
                    5,
                    EventBody::LuaError {
                        status: 1,
                        message: "demo.lua: boom".into(),
                        chunk: "demo.lua".into(),
                        line,
                        traceback: Vec::new(),
                    },
                ),
            ],
        );
        assert_eq!(live.entries.len(), 3);
        assert_eq!(live.fired[&ev].0, 1);
        assert_eq!(live.fired[&send].0, 1);
        assert_eq!(live.entries[2].node, Some(send));
        assert!(live.entries[2].error);
        assert!(live.animating());
        // The breakpoint of a node is its trace text.
        assert_eq!(
            ScriptLive::rule(&g, send),
            BreakOn::Output {
                text: format!("dt:demo:{send};")
            }
        );
    }
}

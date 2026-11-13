//! Messages as they go: "message M from A to B at time t" on a logic graph
//! (stage 35).
//!
//! One form for every source: the running game (the journal of the live
//! link — [`TraceEvent::from_live`]), a run of the logic without the game
//! (stage 38) or a file of such events (JSON lines, [`read_jsonl`],
//! [`write_jsonl`]). [`TraceMap`] finds the nodes and wires of a
//! [`LogicGraph`] an event passes; [`Activity`] adds them up for drawing:
//! per node and wire how often and when last, a feed of the latest events,
//! counts of what did not touch the graph. A busy game sends hundreds of
//! messages a second: an event only bumps counters and times, and the feed
//! keeps the latest [`Activity::FEED`] — drawing reads the totals, so
//! nothing is dropped for being too frequent.
//!
//! The bus of the game names a message and its receivers, not its sender
//! (a SimGroup message has no sender field): for events without
//! [`TraceEvent::sender`] the wires of that message into the receivers are
//! lit and their sources are the likely senders.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

use serde::{Deserialize, Serialize};

use super::{LogicGraph, NodeId, NodeSource, PinKind, WireKind};

/// Who made an event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TraceOrigin {
    /// The game's own bus.
    Game,
    /// Sent by the live link (`live send`).
    Link,
    /// A run of the logic without the game.
    Simulation,
}

/// "Message `hash` from `sender` to `receivers` at `time_ms`".
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TraceEvent {
    /// Number of the event in its source (the journal's number for the
    /// game), increasing.
    pub seq: u64,
    /// Game time in milliseconds.
    pub time_ms: u64,
    /// Frame of the game's main loop, or step of a run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frame: Option<u32>,
    /// Hash of the message name (`sdbm(lower(name))`).
    pub hash: u32,
    /// The name, when the source knows it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// GUID of the sending object (32 hex digits, memory order), when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sender: Option<String>,
    /// GUIDs of the objects that got it (the game lists at most eight).
    #[serde(default)]
    pub receivers: Vec<String>,
    /// Listeners of every kind that got it (engine systems, Lua scripts
    /// too); at least `receivers.len()`.
    #[serde(default)]
    pub listeners: u32,
    /// How many times it went in this event: 1, or more for a count of
    /// messages the game did not record one by one.
    #[serde(default = "one")]
    pub count: u32,
    pub origin: TraceOrigin,
}

fn one() -> u32 {
    1
}

impl TraceEvent {
    /// The message event of the live link's journal (and the counts of
    /// messages over its rate limit, with no receivers); `None` for other
    /// events.
    pub fn from_live(e: &crate::live::proto::Event) -> Vec<Self> {
        use crate::live::proto::journal::{EventBody, MessageOrigin};
        match &e.body {
            EventBody::Message {
                hash,
                listeners,
                receivers,
                origin,
                ..
            } => vec![Self {
                seq: e.seq,
                time_ms: u64::from(e.sim_time_ms),
                frame: Some(e.frame),
                hash: *hash,
                name: None,
                sender: None,
                receivers: receivers.iter().map(crate::live::proto::guid_hex).collect(),
                listeners: *listeners,
                count: 1,
                origin: match origin {
                    MessageOrigin::Game => TraceOrigin::Game,
                    MessageOrigin::Link => TraceOrigin::Link,
                },
            }],
            EventBody::MessageCounts { counts, .. } => counts
                .iter()
                .map(|&(hash, n)| Self {
                    seq: e.seq,
                    time_ms: u64::from(e.sim_time_ms),
                    frame: Some(e.frame),
                    hash,
                    name: None,
                    sender: None,
                    receivers: Vec::new(),
                    listeners: 0,
                    count: n,
                    origin: TraceOrigin::Game,
                })
                .collect(),
            _ => Vec::new(),
        }
    }

    /// Whether this is a count without receivers (see [`Self::count`]):
    /// every wire of its message is lit.
    pub fn is_count(&self) -> bool {
        self.receivers.is_empty() && self.listeners == 0
    }
}

/// Events as JSON lines.
pub fn write_jsonl(events: &[TraceEvent], out: &mut impl std::io::Write) -> std::io::Result<()> {
    for e in events {
        serde_json::to_writer(&mut *out, e)?;
        out.write_all(b"\n")?;
    }
    Ok(())
}

/// Reads JSON lines of events; empty lines are skipped.
pub fn read_jsonl(text: &str) -> serde_json::Result<Vec<TraceEvent>> {
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .map(serde_json::from_str)
        .collect()
}

/// What one event lit on a graph.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Hits {
    /// Nodes that got the message.
    pub received: Vec<NodeId>,
    /// Nodes that sent it — the sender when known, else the sources of the
    /// lit wires (the likely senders).
    pub sent: Vec<NodeId>,
    /// Indices of [`LogicGraph::wires`] the message went along.
    pub wires: Vec<usize>,
}

impl Hits {
    pub fn is_empty(&self) -> bool {
        self.received.is_empty() && self.sent.is_empty() && self.wires.is_empty()
    }
}

/// The nodes and wires of a graph by GUID and message, to match events.
#[derive(Debug, Clone, Default)]
pub struct TraceMap {
    by_guid: HashMap<String, Vec<NodeId>>,
    wires_by_hash: HashMap<u32, Vec<usize>>,
    hubs: HashMap<u32, NodeId>,
    engine: Option<NodeId>,
    /// Nodes with an `out` pin of a message: its possible senders when no
    /// wire of the graph explains an event (the listeners are elsewhere —
    /// the engine, a script, another stream).
    senders_by_hash: HashMap<u32, Vec<NodeId>>,
}

impl TraceMap {
    pub fn new(graph: &LogicGraph) -> Self {
        let mut map = Self::default();
        for (i, n) in graph.nodes.iter().enumerate() {
            if let Some(g) = &n.guid {
                map.by_guid
                    .entry(g.to_ascii_lowercase())
                    .or_default()
                    .push(i);
            }
            for p in &n.pins {
                if let (PinKind::Out, Some(h), NodeSource::Object { .. }) =
                    (p.kind, p.hash, &n.source)
                {
                    let list = map.senders_by_hash.entry(h).or_default();
                    if list.last() != Some(&i) {
                        list.push(i);
                    }
                }
            }
            match n.source {
                NodeSource::Hub { message } => {
                    map.hubs.insert(message, i);
                }
                NodeSource::Engine => map.engine = Some(i),
                NodeSource::Object { .. } => {}
            }
        }
        for (i, w) in graph.wires.iter().enumerate() {
            if let (Some(h), WireKind::Message | WireKind::Engine) = (w.message, w.kind) {
                map.wires_by_hash.entry(h).or_default().push(i);
            }
        }
        map
    }

    /// Nodes of an object GUID (a GUID can be in two SimGroups).
    pub fn nodes_of(&self, guid: &str) -> &[NodeId] {
        self.by_guid
            .get(&guid.to_ascii_lowercase())
            .map_or(&[], Vec::as_slice)
    }

    /// Whether the graph has a wire or a node of this message at all.
    pub fn knows_message(&self, hash: u32) -> bool {
        self.wires_by_hash.contains_key(&hash) || self.senders_by_hash.contains_key(&hash)
    }

    /// What `e` lit on `graph` (the graph this map was made from).
    pub fn hits(&self, graph: &LogicGraph, e: &TraceEvent) -> Hits {
        let mut out = Hits::default();
        let received: HashSet<NodeId> = e
            .receivers
            .iter()
            .flat_map(|g| self.nodes_of(g).iter().copied())
            .collect();
        let senders: Option<HashSet<NodeId>> = e
            .sender
            .as_ref()
            .map(|g| self.nodes_of(g).iter().copied().collect());
        let hub = self.hubs.get(&e.hash).copied();
        // Listeners beyond the objects listed: the engine, or objects past
        // the eight the game lists.
        let more = e.listeners as usize > e.receivers.len() || e.is_count();
        let mut sent: HashSet<NodeId> = HashSet::new();
        let mut hub_lit = false;
        let wires = self
            .wires_by_hash
            .get(&e.hash)
            .map_or(&[][..], Vec::as_slice);
        for &wi in wires {
            let w = &graph.wires[wi];
            let from_ok = senders.as_ref().is_none_or(|s| {
                s.contains(&w.from.node)
                    || Some(w.from.node) == hub
                    || Some(w.from.node) == self.engine
            });
            let into_hub = Some(w.to.node) == hub;
            let to_ok = received.contains(&w.to.node)
                || into_hub
                || (more && Some(w.to.node) == self.engine)
                || (e.is_count() && w.kind == WireKind::Message);
            if from_ok && to_ok {
                out.wires.push(wi);
                if Some(w.from.node) != hub && Some(w.from.node) != self.engine {
                    sent.insert(w.from.node);
                }
                hub_lit |= into_hub || Some(w.from.node) == hub;
            }
        }
        // A hub only passes on: drop wires into it when nothing came out
        // of it to a receiver and the receivers are known.
        if let Some(h) = hub
            && !e.receivers.is_empty()
            && !out.wires.iter().any(|&wi| graph.wires[wi].from.node == h)
        {
            out.wires.retain(|&wi| graph.wires[wi].to.node != h);
            hub_lit = false;
            sent = out
                .wires
                .iter()
                .map(|&wi| graph.wires[wi].from.node)
                .filter(|n| Some(*n) != self.engine)
                .collect();
        }
        if let Some(s) = &senders {
            sent.extend(s.iter().copied());
        } else if out.wires.is_empty()
            && let Some(s) = self.senders_by_hash.get(&e.hash)
        {
            // No wire explains it: the nodes that can send it.
            sent.extend(s.iter().copied());
        }
        out.received = received.into_iter().collect();
        if hub_lit && let Some(h) = hub {
            out.received.push(h);
        }
        out.received.sort_unstable();
        out.sent = sent.into_iter().collect();
        out.sent.sort_unstable();
        out.wires.sort_unstable();
        out.wires.dedup();
        out
    }
}

/// How often and when last.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Stat {
    pub count: u64,
    /// Time of the last event, ms (the event's game time).
    pub last_ms: u64,
    /// Number of the last event that lit it.
    pub last_seq: u64,
}

impl Stat {
    fn bump(&mut self, e: &TraceEvent) {
        self.count += u64::from(e.count.max(1));
        self.last_ms = e.time_ms;
        self.last_seq = e.seq;
    }
}

/// Totals of a stream of events on one graph.
#[derive(Debug, Clone, Default)]
pub struct Activity {
    pub map: TraceMap,
    pub nodes_received: HashMap<NodeId, Stat>,
    pub nodes_sent: HashMap<NodeId, Stat>,
    pub wires: HashMap<usize, Stat>,
    /// Per message hash: how often, and whether it touched the graph.
    pub messages: BTreeMap<u32, (Stat, bool)>,
    /// The latest events, newest last, with what they lit.
    pub feed: VecDeque<(TraceEvent, Hits)>,
    /// Events seen, of them those that touched the graph.
    pub seen: u64,
    pub matched: u64,
    /// Events the source lost (the journal's count).
    pub lost: u64,
    /// The newest event time, ms.
    pub now_ms: u64,
}

impl Activity {
    /// Events the feed keeps.
    pub const FEED: usize = 400;

    pub fn new(graph: &LogicGraph) -> Self {
        Self {
            map: TraceMap::new(graph),
            ..Self::default()
        }
    }

    /// Forgets the totals, keeps the map.
    pub fn clear(&mut self) {
        let map = std::mem::take(&mut self.map);
        *self = Self {
            map,
            ..Self::default()
        };
    }

    /// Adds an event; returns what it lit. `keep` decides whether it goes
    /// to the feed and the totals (a filter of the view: only the
    /// selection, only messages of the graph).
    pub fn add(
        &mut self,
        graph: &LogicGraph,
        e: TraceEvent,
        keep: &dyn Fn(&TraceEvent, &Hits) -> bool,
    ) -> Option<Hits> {
        let hits = self.map.hits(graph, &e);
        if !keep(&e, &hits) {
            return None;
        }
        self.seen += 1;
        self.now_ms = self.now_ms.max(e.time_ms);
        let touched = !hits.is_empty();
        if touched {
            self.matched += 1;
        }
        for &n in &hits.received {
            self.nodes_received.entry(n).or_default().bump(&e);
        }
        for &n in &hits.sent {
            self.nodes_sent.entry(n).or_default().bump(&e);
        }
        for &w in &hits.wires {
            self.wires.entry(w).or_default().bump(&e);
        }
        let m = self.messages.entry(e.hash).or_default();
        m.0.bump(&e);
        m.1 |= touched;
        self.feed.push_back((e, hits.clone()));
        while self.feed.len() > Self::FEED {
            self.feed.pop_front();
        }
        Some(hits)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::logic_graph::{Message, MessageState, Node, Pin, PinKind, PinRef, Wire};

    fn node(guid: &str, source: NodeSource) -> Node {
        Node {
            source,
            stream: String::new(),
            class: 0,
            class_name: "MessageRelay".into(),
            category: crate::logic_graph::Category::Relay,
            name: guid.into(),
            guid: Some(guid.into()),
            position: None,
            pins: vec![
                Pin {
                    slot: 0,
                    class: 0,
                    index: 0,
                    name: "in".into(),
                    kind: PinKind::In,
                    conf: None,
                    message: None,
                    hash: None,
                    delay: None,
                    target: None,
                    value: None,
                },
                Pin {
                    slot: 0,
                    class: 0,
                    index: 1,
                    name: "out".into(),
                    kind: PinKind::Out,
                    conf: None,
                    message: None,
                    hash: None,
                    delay: None,
                    target: None,
                    value: None,
                },
            ],
        }
    }

    fn object(guid: &str, i: usize) -> Node {
        node(
            guid,
            NodeSource::Object {
                group: crate::level::Location {
                    stream: 0,
                    resource: 0,
                },
                object: i,
            },
        )
    }

    fn wire(from: usize, to: usize, h: u32) -> Wire {
        Wire {
            from: PinRef { node: from, pin: 1 },
            to: PinRef { node: to, pin: 0 },
            kind: WireKind::Message,
            message: Some(h),
        }
    }

    /// trigger (a) → `ping` → relays b and c; b → `pong` → c.
    fn graph() -> LogicGraph {
        let mut g = LogicGraph {
            nodes: vec![
                object("aa", 0),
                object("bb", 1),
                object("cc", 2),
                node("", NodeSource::Engine),
            ],
            wires: vec![wire(0, 1, 1), wire(0, 2, 1), wire(1, 2, 2), wire(3, 0, 3)],
            ..LogicGraph::default()
        };
        g.nodes[3].guid = None;
        g.wires[3].kind = WireKind::Engine;
        for h in [1, 2, 3] {
            g.messages.insert(
                h,
                Message {
                    hash: h,
                    name: format!("m{h}"),
                    senders: Vec::new(),
                    listeners: Vec::new(),
                    state: MessageState::Connected,
                    engine: h == 3,
                    hub: None,
                    elsewhere: None,
                },
            );
        }
        g
    }

    fn event(seq: u64, hash: u32, receivers: &[&str]) -> TraceEvent {
        TraceEvent {
            seq,
            time_ms: seq * 100,
            frame: None,
            hash,
            name: None,
            sender: None,
            receivers: receivers.iter().map(|s| (*s).to_owned()).collect(),
            listeners: receivers.len() as u32,
            count: 1,
            origin: TraceOrigin::Game,
        }
    }

    #[test]
    fn a_message_lights_its_wires_into_the_receivers() {
        let g = graph();
        let map = TraceMap::new(&g);
        // `ping` heard by b only: the wire a → b, a is the sender.
        let h = map.hits(&g, &event(1, 1, &["BB"]));
        assert_eq!(h.received, [1]);
        assert_eq!(h.sent, [0]);
        assert_eq!(h.wires, [0]);
        // Heard by both.
        let h = map.hits(&g, &event(2, 1, &["bb", "cc"]));
        assert_eq!(h.wires, [0, 1]);
        // A known sender narrows the wires.
        let mut e = event(3, 2, &["cc"]);
        e.sender = Some("bb".into());
        let h = map.hits(&g, &e);
        assert_eq!(
            (h.sent.as_slice(), h.wires.as_slice()),
            (&[1][..], &[2][..])
        );
        e.sender = Some("aa".into());
        assert!(map.hits(&g, &e).wires.is_empty());
        // An engine message: from the engine node into a.
        let h = map.hits(&g, &event(4, 3, &["aa"]));
        assert_eq!(
            (h.received.as_slice(), h.wires.as_slice()),
            (&[0][..], &[3][..])
        );
        assert!(h.sent.is_empty());
        // Not in the graph.
        assert!(map.hits(&g, &event(5, 99, &["zz"])).is_empty());
        // Sent by a node of the graph to listeners elsewhere: the node that
        // can send it is the sender.
        let mut g2 = graph();
        g2.nodes[2].pins[1].hash = Some(42);
        g2.nodes[2].pins[1].message = Some("aud_enter".into());
        let map2 = TraceMap::new(&g2);
        let mut e = event(6, 42, &[]);
        e.listeners = 5;
        let h = map2.hits(&g2, &e);
        assert_eq!(h.sent, [2]);
        assert!(h.wires.is_empty() && h.received.is_empty());
        assert!(map2.knows_message(42));
    }

    #[test]
    fn activity_adds_up_and_keeps_a_feed() {
        let g = graph();
        let mut a = Activity::new(&g);
        for i in 0..1000 {
            a.add(&g, event(i, 1, &["bb", "cc"]), &|_, _| true);
        }
        // A count of messages over the game's rate limit: their wires,
        // without receivers.
        let mut counted = event(1000, 1, &[]);
        counted.count = 500;
        counted.listeners = 0;
        assert!(counted.is_count());
        a.add(&g, counted, &|_, _| true);
        a.add(&g, event(1001, 99, &["zz"]), &|_, _| true);
        assert_eq!(a.seen, 1002);
        assert_eq!(a.matched, 1001);
        assert_eq!(a.wires[&0].count, 1500);
        assert_eq!(a.nodes_received[&1].count, 1000);
        assert_eq!(a.nodes_sent[&0].count, 1500);
        assert_eq!(a.feed.len(), Activity::FEED);
        assert_eq!(a.messages[&99], (a.messages[&99].0, false));
        // A filter keeps only what touches node c.
        let mut only_c = Activity::new(&g);
        let keep = |_: &TraceEvent, h: &Hits| h.received.contains(&2) || h.sent.contains(&2);
        only_c.add(&g, event(1, 1, &["bb"]), &keep);
        only_c.add(&g, event(2, 2, &["cc"]), &keep);
        assert_eq!(only_c.seen, 1);
        a.clear();
        assert_eq!(a.seen, 0);
        assert!(a.map.knows_message(1));
    }

    #[test]
    fn events_round_trip_as_json_lines() {
        let mut e = event(7, 1, &["bb"]);
        e.name = Some("ping".into());
        e.sender = Some("aa".into());
        e.frame = Some(3);
        let mut out = Vec::new();
        write_jsonl(&[e.clone(), event(8, 2, &[])], &mut out).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert_eq!(text.lines().count(), 2);
        assert_eq!(read_jsonl(&text).unwrap()[0], e);
        // Fields a simulation may leave out.
        let short = read_jsonl(r#"{"seq":1,"time_ms":0,"hash":5,"origin":"simulation"}"#).unwrap();
        assert_eq!(short[0].count, 1);
        assert!(short[0].receivers.is_empty());
    }

    #[test]
    fn journal_events_become_trace_events() {
        use crate::live::proto::journal::{EventBody, MessageOrigin};
        let e = crate::live::proto::Event {
            seq: 4,
            frame: 10,
            sim_time_ms: 1500,
            body: EventBody::Message {
                hash: 1,
                data: 0,
                forced: false,
                listeners: 3,
                receivers: vec![[0xbb; 16]],
                origin: MessageOrigin::Link,
            },
        };
        let t = TraceEvent::from_live(&e);
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].receivers[0], "bb".repeat(16));
        assert_eq!(t[0].origin, TraceOrigin::Link);
        let counts = crate::live::proto::Event {
            body: EventBody::MessageCounts {
                window_ms: 1000,
                counts: vec![(1, 60), (2, 5)],
            },
            ..e
        };
        let t = TraceEvent::from_live(&counts);
        assert_eq!(t.len(), 2);
        assert!(t.iter().all(TraceEvent::is_count));
    }
}

//! The game's journal (stage 49): what the game prints, the errors of its
//! Lua scripts and the messages of the SimGroup bus, as numbered events.
//!
//! The module keeps the latest events in a ring ([`Ring`]); a client asks
//! for the events after the last number it has seen
//! ([`crate::Request::Events`]) and learns how many it lost when the ring
//! went round in between. Each kind has a rate limit: a script that fails
//! every frame or a busy message bus fills the ring with the first events
//! of every second and counts the rest as lost.

use std::collections::VecDeque;

use crate::Guid;

/// Kinds of events, as bits of a mask.
pub mod mask {
    /// What the game prints: `OutputDebugStringA`, Lua `print`.
    pub const OUTPUT: u8 = 1;
    /// Errors of Lua code: compile and run time, with the call stack.
    pub const LUA_ERROR: u8 = 2;
    /// Messages of the SimGroup bus with their receivers.
    pub const MESSAGE: u8 = 4;
    /// Protocol 3 (stage 35): once a second, how many messages of each
    /// hash the bus delivered beyond those recorded one by one (the rate
    /// limit per message keeps their events out, not their count).
    pub const MESSAGE_COUNTS: u8 = 8;
    /// The kinds of protocol 2.
    pub const ALL: u8 = OUTPUT | LUA_ERROR | MESSAGE;
    /// The kinds of protocol 3.
    pub const ALL_V3: u8 = ALL | MESSAGE_COUNTS;

    /// The kinds a client of protocol `version` may get: a module answers
    /// older clients without the newer kinds, which they cannot read.
    pub fn for_version(kinds: u8, version: u16) -> u8 {
        if version >= 3 {
            kinds & ALL_V3
        } else {
            kinds & ALL
        }
    }
}

/// Where printed text comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputSource {
    /// `OutputDebugStringA` of the game (`0x0065BC40` and others).
    Debug = 1,
    /// Lua `print` of a script (`0x0046BCE0`).
    Print = 2,
    /// A note of the module itself.
    Module = 3,
}

impl OutputSource {
    pub fn from_u8(v: u8) -> Self {
        match v {
            1 => Self::Debug,
            2 => Self::Print,
            _ => Self::Module,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Debug => "debug",
            Self::Print => "print",
            Self::Module => "module",
        }
    }
}

/// Who sent a message of the bus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageOrigin {
    /// The game: its bus `0x00408A00`.
    Game = 0,
    /// The live link (`live send`).
    Link = 1,
}

/// What happened.
#[derive(Debug, Clone, PartialEq)]
pub enum EventBody {
    /// A line the game printed.
    Output {
        source: OutputSource,
        text: String,
        /// The script and line of a Lua `print` (empty and 0 otherwise).
        script: String,
        line: i32,
    },
    /// Lua code failed. `status` is Lua's: 1 run time, 3 syntax, 4 memory,
    /// 5 error in the error handler.
    LuaError {
        status: u8,
        message: String,
        /// Chunk of the innermost Lua function (`@scripts/…`, `=doctor_live`).
        chunk: String,
        /// Its line, -1 when unknown.
        line: i32,
        /// One line per level of the Lua call stack, innermost first.
        traceback: Vec<String>,
    },
    /// A message went through the bus.
    Message {
        hash: u32,
        data: u32,
        /// Sent to every listener, switched on or not.
        forced: bool,
        /// Listeners the bus calls.
        listeners: u32,
        /// GUIDs of the SimGroup objects among them (at most
        /// [`MAX_RECEIVERS`]).
        receivers: Vec<Guid>,
        origin: MessageOrigin,
    },
    /// Protocol 3: messages of these hashes that went through the bus in
    /// the last `window_ms` without an event of their own, and how many.
    MessageCounts {
        window_ms: u32,
        counts: Vec<(u32, u32)>,
    },
}

impl EventBody {
    /// The bit of [`mask`] of this kind.
    pub fn mask(&self) -> u8 {
        match self {
            Self::Output { .. } => mask::OUTPUT,
            Self::LuaError { .. } => mask::LUA_ERROR,
            Self::Message { .. } => mask::MESSAGE,
            Self::MessageCounts { .. } => mask::MESSAGE_COUNTS,
        }
    }
}

/// One numbered event.
#[derive(Debug, Clone, PartialEq)]
pub struct Event {
    /// Number of the event: 1, 2, … since the module was loaded.
    pub seq: u64,
    /// Frame of the main loop when it happened.
    pub frame: u32,
    /// Game time in milliseconds, 0 outside a level.
    pub sim_time_ms: u32,
    pub body: EventBody,
}

/// Answer to [`crate::Request::Events`].
#[derive(Debug, Clone, PartialEq, Default)]
pub struct EventBatch {
    /// Ask with this number next time.
    pub next: u64,
    /// Events lost since the asked number: the ring went round, or a rate
    /// limit dropped them.
    pub lost: u64,
    /// Kinds the module records ([`mask`]): hooks it could install.
    pub recording: u8,
    pub events: Vec<Event>,
}

/// Longest text of an event, in bytes.
pub const MAX_TEXT: usize = 2048;
/// Lines of a traceback kept.
pub const MAX_TRACEBACK: usize = 16;
/// Receivers of a message listed.
pub const MAX_RECEIVERS: usize = 8;
/// Events a ring keeps.
pub const RING_SIZE: usize = 4096;
/// Events of one kind recorded per second; the rest are counted as lost.
pub const RATE_OUTPUT: u32 = 200;
pub const RATE_LUA_ERROR: u32 = 50;
pub const RATE_MESSAGE: u32 = 300;
pub const RATE_MESSAGE_COUNTS: u32 = 20;
/// Hashes one [`EventBody::MessageCounts`] lists at most.
pub const MAX_COUNTS: usize = 512;

/// Counts of messages that passed the bus without an event of their own
/// (over the rate limit of their hash), gathered between two
/// [`EventBody::MessageCounts`].
#[derive(Debug, Clone, Default)]
pub struct Counts {
    counts: std::collections::BTreeMap<u32, u32>,
}

impl Counts {
    pub fn add(&mut self, hash: u32) {
        if self.counts.len() < MAX_COUNTS || self.counts.contains_key(&hash) {
            *self.counts.entry(hash).or_default() += 1;
        }
    }

    pub fn is_empty(&self) -> bool {
        self.counts.is_empty()
    }

    /// The event for the last `window_ms`, and a fresh start.
    pub fn take(&mut self, window_ms: u32) -> Option<EventBody> {
        if self.counts.is_empty() {
            return None;
        }
        let counts = std::mem::take(&mut self.counts).into_iter().collect();
        Some(EventBody::MessageCounts { window_ms, counts })
    }
}

/// `text` cut to [`MAX_TEXT`] bytes on a character boundary, without a line
/// break at its end.
pub fn clip(text: &str) -> String {
    let t = text.trim_end_matches(['\r', '\n']);
    let mut end = t.len().min(MAX_TEXT);
    while !t.is_char_boundary(end) {
        end -= 1;
    }
    t[..end].to_owned()
}

/// The ring of the module: the latest events, numbered, with rate limits.
#[derive(Debug, Clone)]
pub struct Ring {
    events: VecDeque<Event>,
    capacity: usize,
    next: u64,
    /// Events dropped by rate limits (numbered too, so a client sees the
    /// gap).
    dropped: VecDeque<u64>,
    /// Per kind: the second and the count of events in it.
    window: [(u64, u32); 4],
}

impl Default for Ring {
    fn default() -> Self {
        Self::new(RING_SIZE)
    }
}

impl Ring {
    pub fn new(capacity: usize) -> Self {
        Self {
            events: VecDeque::with_capacity(capacity.min(RING_SIZE)),
            capacity: capacity.max(1),
            next: 1,
            dropped: VecDeque::new(),
            window: [(u64::MAX, 0); 4],
        }
    }

    /// Number the next event will get.
    pub fn next_seq(&self) -> u64 {
        self.next
    }

    fn slot(kind: u8) -> (usize, u32) {
        match kind {
            mask::OUTPUT => (0, RATE_OUTPUT),
            mask::LUA_ERROR => (1, RATE_LUA_ERROR),
            mask::MESSAGE_COUNTS => (3, RATE_MESSAGE_COUNTS),
            _ => (2, RATE_MESSAGE),
        }
    }

    /// Whether an event of `kind` ([`mask`]) at `second` would pass the
    /// rate limit — to skip the work of making one that would not. Then
    /// [`Ring::lose`] counts it.
    pub fn admits(&self, second: u64, kind: u8) -> bool {
        let (slot, limit) = Self::slot(kind);
        let w = self.window[slot];
        w.0 != second || w.1 < limit
    }

    /// Counts an event of `kind` that was not made (over the rate limit).
    pub fn lose(&mut self, second: u64, kind: u8) {
        let (slot, _) = Self::slot(kind);
        let seq = self.next;
        self.next += 1;
        let w = &mut self.window[slot];
        if w.0 != second {
            *w = (second, 0);
        }
        w.1 += 1;
        self.dropped.push_back(seq);
        if self.dropped.len() > self.capacity {
            self.dropped.pop_front();
        }
    }

    /// Records an event at `second` (any clock in seconds; used by the rate
    /// limit). Returns its number, or `None` when the rate limit dropped it.
    pub fn push(
        &mut self,
        second: u64,
        frame: u32,
        sim_time_ms: u32,
        body: EventBody,
    ) -> Option<u64> {
        let kind = body.mask();
        if !self.admits(second, kind) {
            self.lose(second, kind);
            return None;
        }
        let (slot, _) = Self::slot(kind);
        let seq = self.next;
        self.next += 1;
        let w = &mut self.window[slot];
        if w.0 != second {
            *w = (second, 0);
        }
        w.1 += 1;
        if self.events.len() == self.capacity {
            self.events.pop_front();
        }
        self.events.push_back(Event {
            seq,
            frame,
            sim_time_ms,
            body,
        });
        Some(seq)
    }

    /// Events numbered `after + 1` and on whose kind is in `kinds`, at most
    /// `max`; `lost` counts the numbers in that range that are gone (rate
    /// limit, or overwritten) — events of other kinds are skipped, not
    /// lost.
    pub fn after(&self, after: u64, kinds: u8, max: usize) -> EventBatch {
        let mut batch = EventBatch {
            next: self.next.max(after.saturating_add(1)),
            ..EventBatch::default()
        };
        let first_kept = self.events.front().map_or(self.next, |e| e.seq);
        let mut expected = after.saturating_add(1);
        if expected < first_kept {
            // Overwritten by the ring (dropped ones are counted below).
            let gone = first_kept - expected;
            let dropped_there = self
                .dropped
                .iter()
                .filter(|&&s| s >= expected && s < first_kept)
                .count() as u64;
            batch.lost += gone - dropped_there.min(gone);
            expected = first_kept;
        }
        for e in self.events.iter().filter(|e| e.seq >= expected) {
            if batch.events.len() >= max {
                batch.next = e.seq;
                break;
            }
            if e.body.mask() & kinds != 0 {
                batch.events.push(e.clone());
            }
        }
        let upto = batch.next;
        batch.lost += self
            .dropped
            .iter()
            .filter(|&&s| s > after && s < upto)
            .count() as u64;
        batch
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn out(text: &str) -> EventBody {
        EventBody::Output {
            source: OutputSource::Debug,
            text: text.into(),
            script: String::new(),
            line: 0,
        }
    }

    fn message(hash: u32) -> EventBody {
        EventBody::Message {
            hash,
            data: 0,
            forced: false,
            listeners: 1,
            receivers: vec![[1; 16]],
            origin: MessageOrigin::Game,
        }
    }

    fn texts(b: &EventBatch) -> Vec<String> {
        b.events
            .iter()
            .map(|e| match &e.body {
                EventBody::Output { text, .. } => text.clone(),
                other => format!("{other:?}"),
            })
            .collect()
    }

    #[test]
    fn a_client_reads_on_from_where_it_stopped() {
        let mut r = Ring::new(8);
        for i in 0..5 {
            r.push(0, i, 0, out(&format!("line {i}")));
        }
        let b = r.after(0, mask::ALL, 100);
        assert_eq!(b.events.len(), 5);
        assert_eq!(b.next, 6);
        assert_eq!(b.lost, 0);
        assert_eq!(b.events[0].seq, 1);
        // Nothing new.
        let b2 = r.after(b.next - 1, mask::ALL, 100);
        assert!(b2.events.is_empty());
        assert_eq!(b2.next, 6);
        // Two more.
        r.push(1, 9, 0, out("six"));
        r.push(1, 9, 0, out("seven"));
        let b3 = r.after(5, mask::ALL, 100);
        assert_eq!(texts(&b3), ["six", "seven"]);
        // `max` cuts the batch; the rest comes next time.
        let b4 = r.after(0, mask::ALL, 3);
        assert_eq!(b4.events.len(), 3);
        assert_eq!(b4.next, 4);
        assert_eq!(r.after(b4.next - 1, mask::ALL, 100).events.len(), 4);
    }

    #[test]
    fn the_ring_goes_round_and_says_how_much_was_lost() {
        let mut r = Ring::new(4);
        for i in 0..10 {
            r.push(0, 0, 0, out(&format!("{i}")));
        }
        let b = r.after(0, mask::ALL, 100);
        assert_eq!(texts(&b), ["6", "7", "8", "9"]);
        assert_eq!(b.lost, 6);
        assert_eq!(b.next, 11);
    }

    #[test]
    fn rate_limits_drop_and_count() {
        let mut r = Ring::new(RING_SIZE);
        for _ in 0..(RATE_MESSAGE + 10) {
            r.push(7, 0, 0, message(0xAB));
        }
        // Output has its own limit.
        assert!(r.push(7, 0, 0, out("still here")).is_some());
        let b = r.after(0, mask::ALL, 10_000);
        assert_eq!(b.events.len() as u32, RATE_MESSAGE + 1);
        assert_eq!(b.lost, 10);
        // The next second starts again.
        assert!(r.push(8, 0, 0, message(0xAB)).is_some());
    }

    #[test]
    fn kinds_filter_without_counting_as_lost() {
        let mut r = Ring::new(16);
        r.push(0, 0, 0, out("a"));
        r.push(0, 0, 0, message(1));
        r.push(0, 0, 0, out("b"));
        let b = r.after(0, mask::OUTPUT, 100);
        assert_eq!(texts(&b), ["a", "b"]);
        assert_eq!(b.lost, 0);
        assert_eq!(b.next, 4);
        let m = r.after(0, mask::MESSAGE, 100);
        assert_eq!(m.events.len(), 1);
    }

    #[test]
    fn counts_of_messages_over_the_limit() {
        let mut c = Counts::default();
        assert!(c.take(1000).is_none());
        for _ in 0..1000 {
            c.add(0xAB);
        }
        c.add(0xCD);
        let e = c.take(1000).unwrap();
        assert_eq!(
            e,
            EventBody::MessageCounts {
                window_ms: 1000,
                counts: vec![(0xAB, 1000), (0xCD, 1)],
            }
        );
        assert!(c.is_empty());
        // Older clients do not get them.
        assert_eq!(mask::for_version(0xFF, 2), mask::ALL);
        assert_eq!(mask::for_version(0xFF, 3), mask::ALL_V3);
        let mut r = Ring::new(16);
        r.push(0, 0, 0, e);
        r.push(0, 0, 0, message(1));
        assert_eq!(r.after(0, mask::ALL, 100).events.len(), 1);
        assert_eq!(r.after(0, mask::ALL_V3, 100).events.len(), 2);
    }

    #[test]
    fn long_text_is_clipped_on_a_character() {
        let long = "й".repeat(MAX_TEXT);
        let c = clip(&long);
        assert!(c.len() <= MAX_TEXT);
        assert!(c.chars().all(|ch| ch == 'й'));
        assert_eq!(clip("line\r\n"), "line");
    }
}

//! Running the logic of a scope without the game (stage 38).
//!
//! Objects of the SimGroups are actors. A message goes through a bus like
//! the game's (`0x00408A00`): every object that listens to the name gets
//! it, synchronously and depth first — a reaction that sends at once is
//! handled before the next listener gets the first message. Delays and
//! timers go to a queue ordered by game time. Classes whose handler was
//! read in `godfather2.exe` have a model ([`Model`]); every other object is
//! a black box: the message reaches it, the reaction is not known, and the
//! user may give one ([`ManualReaction`]). `LuaAudioScript` objects run
//! their chunks on the Lua 5.0 VM with the mock of the game's API
//! ([`crate::script::sim`]). The run is deterministic: the same input gives
//! the same journal ([`Entry`]).
//!
//! See `docs/logic-graph.md` §9.

pub mod journal;
mod model;
mod spec;

#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::game::{GameInstance, ResourceIndex};
use crate::hash::string_id;
use crate::level::Location;
use crate::resource::lua::TYPE_LUA;
use crate::resource::lua::vm::Value as LuaValue;
use crate::resource::simgroup::{Guid, Value};
use crate::script::sim::{Sim, SimEvent};
use crate::workspace::{ResourceKey, Workspace};
use crate::{Error, Result};

use super::scope::{self, LoadedGroup};
use super::{LogicSchema, PinKind, ScopeSpec};

pub use spec::RunSpec;

pub use journal::{
    Comparison, Entry, GameMessage, Matched, Took, What, Who, compare, parse_game_journal,
};
pub use model::{Model, Msg, Out, POST_SPAWN, Switch, TimerState};

use model::{Action, Attrs, Context};

/// An object of the run.
#[derive(Debug, Clone, Serialize)]
pub struct Actor {
    #[serde(serialize_with = "ser_location")]
    pub location: Location,
    /// Index of the object in its SimGroup.
    pub object: usize,
    pub stream: String,
    pub guid: Guid,
    pub class: String,
    /// `CMD_SetName`, empty when none.
    pub name: String,
    /// Messages it listens to and the pins they come in by.
    pub listens: Vec<(Msg, String)>,
    pub model: Model,
    /// Reactions given by the user.
    pub manual: Vec<ManualSend>,
}

fn ser_location<S: serde::Serializer>(l: &Location, s: S) -> std::result::Result<S::Ok, S::Error> {
    use serde::ser::SerializeStruct;
    let mut st = s.serialize_struct("Location", 2)?;
    st.serialize_field("stream", &l.stream)?;
    st.serialize_field("resource", &l.resource)?;
    st.end()
}

impl Actor {
    /// The name, or the class and the place.
    pub fn label(&self) -> String {
        if self.name.is_empty() {
            let tail = self.stream.rsplit('/').next().unwrap_or(&self.stream);
            format!("{}@{tail}:{}", self.class, self.object)
        } else {
            self.name.clone()
        }
    }

    fn who(&self, index: usize) -> Who {
        Who {
            kind: "object".into(),
            label: self.label(),
            actor: Some(index),
            guid: Some(self.guid.to_string()),
            class: Some(self.class.clone()),
        }
    }
}

/// A reaction the user gives an object (black boxes above all): when it
/// gets `on` (any message when `None`), it sends `send`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ManualReaction {
    /// The object: GUID, name, `#<actor>` or `<stream>#<resource>:<object>`.
    pub object: String,
    #[serde(default)]
    pub on: Option<String>,
    pub send: Vec<ManualOut>,
}

/// A message of a [`ManualReaction`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ManualOut {
    pub message: String,
    #[serde(default)]
    pub delay: f64,
}

/// A manual reaction attached to an actor.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ManualSend {
    pub on: Option<Msg>,
    pub send: Vec<Out>,
}

/// An input of the run: what the game or the player does.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "input", content = "value", rename_all = "snake_case")]
pub enum Input {
    /// The SimGroups are loaded: `LuaAudioScript` objects with flag 1 start
    /// their scripts, then the game sends [`POST_SPAWN`].
    Load,
    /// The game sends a message (`iMsgNewCheckpointLoaded`, …).
    Message(String),
    /// The player enters a trigger volume (object selector).
    Enter(String),
    /// The player leaves a trigger volume.
    Exit(String),
    /// A game variable takes a value.
    Variable(String, i32),
    /// The current checkpoint (for `TestCurrentCheckpoint`).
    Checkpoint(i32),
}

impl Input {
    /// Parses `load`, `send:MSG`, `enter:OBJ`, `exit:OBJ`, `var:NAME=N`,
    /// `checkpoint:N`.
    pub fn parse(text: &str) -> Result<Self> {
        let t = text.trim();
        let bad = || Error::InvalidArgument(format!("bad event {t:?}"));
        if t.eq_ignore_ascii_case("load") {
            return Ok(Self::Load);
        }
        let (k, v) = t.split_once(':').ok_or_else(bad)?;
        let v = v.trim();
        if v.is_empty() {
            return Err(bad());
        }
        Ok(match k.trim().to_ascii_lowercase().as_str() {
            "send" | "message" | "msg" => Self::Message(v.to_owned()),
            "enter" => Self::Enter(v.to_owned()),
            "exit" | "leave" => Self::Exit(v.to_owned()),
            "var" | "variable" => {
                let (n, x) = v.split_once('=').ok_or_else(bad)?;
                Self::Variable(n.trim().to_owned(), x.trim().parse().map_err(|_| bad())?)
            }
            "checkpoint" | "cp" => Self::Checkpoint(v.parse().map_err(|_| bad())?),
            _ => return Err(bad()),
        })
    }

    /// Text as [`Self::parse`] reads it.
    pub fn text(&self) -> String {
        match self {
            Self::Load => "load".into(),
            Self::Message(m) => format!("send:{m}"),
            Self::Enter(o) => format!("enter:{o}"),
            Self::Exit(o) => format!("exit:{o}"),
            Self::Variable(n, v) => format!("var:{n}={v}"),
            Self::Checkpoint(n) => format!("checkpoint:{n}"),
        }
    }
}

/// Parses a list of inputs with times: `wait:S` moves the time on, `at:T:`
/// before an input puts it at time `T`; the others come at the current
/// time. Several may be in one text, separated by `;` or `,`.
pub fn parse_inputs(texts: &[String]) -> Result<Vec<(f64, Input)>> {
    let mut out = Vec::new();
    let mut now = 0.0f64;
    for part in texts
        .iter()
        .flat_map(|t| t.split([';', ',']))
        .map(str::trim)
        .filter(|t| !t.is_empty())
    {
        let lower = part.to_ascii_lowercase();
        if let Some(s) = lower.strip_prefix("wait:") {
            let s: f64 = s
                .trim()
                .parse()
                .map_err(|_| Error::InvalidArgument(format!("bad wait {part:?}")))?;
            now += s.max(0.0);
            continue;
        }
        if lower.starts_with("at:") {
            let rest = &part[3..];
            let (t, input) = rest
                .split_once(':')
                .ok_or_else(|| Error::InvalidArgument(format!("bad event {part:?}")))?;
            let t: f64 = t
                .trim()
                .parse()
                .map_err(|_| Error::InvalidArgument(format!("bad time in {part:?}")))?;
            out.push((t.max(0.0), Input::parse(input)?));
            continue;
        }
        out.push((now, Input::parse(part)?));
    }
    Ok(out)
}

/// A point where [`Run::run`] stops.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(tag = "on", content = "value", rename_all = "snake_case")]
pub enum Breakpoint {
    /// Before the message is sent (by hash).
    Message(u32),
    /// Before the object gets any message.
    Object(usize),
}

/// How far [`Run::run`] goes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Until {
    /// This many steps.
    Steps(usize),
    /// Until a message has been sent.
    Message,
    /// Until game time `t` (seconds).
    Time(f64),
    /// Until nothing is left to do.
    Idle,
}

/// Why [`Run::run`] stopped.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "stop", content = "value", rename_all = "snake_case")]
pub enum Stop {
    Steps,
    Message,
    Time,
    /// Nothing left: no message to deliver, no timer.
    Idle,
    /// A breakpoint, before what it names.
    Breakpoint(String),
    /// The step limit.
    Limit,
}

/// What one step did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepOutcome {
    Did,
    Idle,
    Break,
}

/// Options of a run.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RunOptions {
    /// Seed of `rand()` for `TriggerRandom` (the game's own state is not
    /// known; 1 is `rand()`'s seed at start).
    pub seed: u32,
    /// Synchronous sends deeper than this are a loop without delay (the
    /// game would overflow its stack).
    pub max_depth: u32,
    /// Run `LuaAudioScript` chunks.
    pub lua: bool,
}

impl Default for RunOptions {
    fn default() -> Self {
        Self {
            seed: 1,
            max_depth: 64,
            lua: true,
        }
    }
}

/// What a run starts from.
#[derive(Default)]
pub struct RunInput {
    pub groups: Vec<LoadedGroup>,
    /// Data of `lua` resources by resource id.
    pub chunks: HashMap<u32, Vec<u8>>,
    /// Variables of the variable dictionaries: name, start value.
    pub variables: Vec<(String, i32)>,
}

/// Reads the SimGroups of a scope, the Lua chunks their `LuaAudioScript`
/// objects run and the game's variable dictionaries (the workspace's
/// version where it has one). Groups that fail to read are returned.
pub fn load(
    game: &GameInstance,
    index: &ResourceIndex,
    ws: Option<&Workspace>,
    spec: &ScopeSpec,
) -> Result<(RunInput, Vec<(Location, Error)>)> {
    let locations = scope::groups(game, index, spec)?;
    let (groups, errors) = scope::load(game, ws, &locations);
    let mut wanted: HashSet<u32> = HashSet::new();
    for g in &groups {
        for o in &g.group.objects {
            if let Some(Value::Guid(id)) = g.group.attr(o, model::class::LUA_AUDIO_SCRIPT, 3)
                && let Some(id) = id.resource_id()
            {
                wanted.insert(id);
            }
        }
    }
    let mut chunks = HashMap::new();
    let dict_type = crate::resource::logic::TYPE_VARIABLE_DICTIONARY;
    let mut variables = Vec::new();
    let mut seen_vars = HashSet::new();
    let read = |stream: usize,
                resource: usize,
                header: &crate::container::ustream::ResourceHeader|
     -> Option<Vec<u8>> {
        let file = &game.streams()[stream];
        let key = ResourceKey::new(&file.stream_name(), header);
        if let Some(d) = ws.and_then(|w| w.data(&key).ok().flatten()) {
            return Some(d);
        }
        game.open_stream(stream).ok()?.resource_data(resource).ok()
    };
    for e in &index.entries {
        if e.header.type_id == TYPE_LUA && wanted.contains(&e.header.ids[3]) {
            if let Some(d) = read(e.stream as usize, e.resource as usize, &e.header) {
                chunks.entry(e.header.ids[3]).or_insert(d);
            }
        } else if e.header.type_id == dict_type
            && let Some(d) = read(e.stream as usize, e.resource as usize, &e.header)
            && let Ok(dict) = crate::resource::variabledictionary::VariableDictionary::parse(&d)
        {
            for v in dict.variables {
                if seen_vars.insert(v.name.to_ascii_lowercase()) {
                    variables.push((v.name, v.value as i32));
                }
            }
        }
    }
    if let Some(ws) = ws {
        for c in ws.changes() {
            if c.is_added()
                && c.key.type_id == TYPE_LUA
                && wanted.contains(&c.key.ids[3])
                && let Ok(d) = ws.store().get(&c.payload)
            {
                chunks.insert(c.key.ids[3], d);
            }
        }
    }
    Ok((
        RunInput {
            groups,
            chunks,
            variables,
        },
        errors,
    ))
}

#[derive(Debug, Clone)]
struct Variable {
    name: Option<String>,
    value: i32,
    /// In a variable dictionary or set during the run.
    known: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Receiver {
    Actor(usize),
    Lua,
}

#[derive(Debug, Clone)]
enum Work {
    Send {
        from: Who,
        msg: Msg,
        target: Option<Guid>,
        depth: u32,
    },
    Deliver {
        to: Receiver,
        msg: Msg,
        from: Who,
        depth: u32,
    },
    VariableChanged {
        hash: u32,
        depth: u32,
    },
}

#[derive(Debug, Clone)]
enum Pending {
    Send {
        from: Who,
        msg: Msg,
        target: Option<Guid>,
    },
    Expire {
        actor: usize,
        generation: u32,
    },
    Input(Input),
}

/// A run of the logic of a scope.
pub struct Run {
    actors: Vec<Actor>,
    listeners: HashMap<u32, Vec<usize>>,
    /// Message names by hash (as first written in the scope).
    names: HashMap<u32, String>,
    time: f64,
    stack: Vec<Work>,
    pending: BTreeMap<(u64, u64), (f64, Pending)>,
    pending_seq: u64,
    variables: BTreeMap<u32, Variable>,
    checkpoint: Option<i32>,
    seed: u32,
    journal: Vec<Entry>,
    steps: u64,
    breakpoints: HashSet<Breakpoint>,
    lua: Option<Sim>,
    chunks: HashMap<u32, Vec<u8>>,
    options: RunOptions,
}

fn hex(h: u32) -> String {
    format!("{h:08x}")
}

fn time_key(t: f64) -> u64 {
    // Non-negative times: the bits of an f64 order like the numbers.
    t.max(0.0).to_bits()
}

impl Run {
    /// A run of the SimGroups of `input`.
    pub fn new(input: RunInput, options: RunOptions) -> Self {
        let schema = LogicSchema::gf2();
        let mut actors = Vec::new();
        let mut names: HashMap<u32, String> = HashMap::new();
        let mut var_names: Vec<(u32, String)> = Vec::new();
        for g in &input.groups {
            for (i, o) in g.group.objects.iter().enumerate() {
                let class = o.class_name().map_or_else(
                    || {
                        schema
                            .class_name(o.class)
                            .map_or_else(|| format!("{:08x}", o.class), str::to_owned)
                    },
                    str::to_owned,
                );
                for op in schema.object_pins(&g.group, o) {
                    if matches!(op.attr.pin, PinKind::In | PinKind::Out)
                        && let Some(Value::String(s)) = &op.value
                        && !s.is_empty()
                    {
                        names.entry(string_id(s)).or_insert_with(|| s.clone());
                    }
                }
                if let Some(Value::String(s)) = g.group.attr(o, model::class::VARIABLE_SWITCH, 2)
                    && !s.is_empty()
                {
                    var_names.push((string_id(s), s.clone()));
                }
                let a = Attrs {
                    group: &g.group,
                    object: o,
                };
                let model = model::build(&class, &a);
                let mut listens = model::listened(schema, &g.group, o);
                for m in model.implicit() {
                    if !listens.iter().any(|(x, _)| x.hash == m.hash) {
                        listens.push((m, "(implicit)".to_owned()));
                    }
                }
                if listens.is_empty() && model.is_black_box() {
                    continue;
                }
                actors.push(Actor {
                    location: g.location,
                    object: i,
                    stream: g.stream.clone(),
                    guid: o.guid,
                    class,
                    name: g.group.object_name(o).unwrap_or_default().to_owned(),
                    listens,
                    model,
                    manual: Vec::new(),
                });
            }
        }
        let mut listeners: HashMap<u32, Vec<usize>> = HashMap::new();
        for (i, a) in actors.iter().enumerate() {
            for (m, _) in &a.listens {
                listeners.entry(m.hash).or_default().push(i);
            }
        }
        let mut variables = BTreeMap::new();
        for (name, value) in &input.variables {
            variables.insert(
                string_id(name),
                Variable {
                    name: Some(name.clone()),
                    value: *value,
                    known: true,
                },
            );
        }
        for (h, name) in var_names {
            variables
                .entry(h)
                .or_insert(Variable {
                    name: None,
                    value: 0,
                    known: false,
                })
                .name
                .get_or_insert(name);
        }
        let lua = options.lua.then(|| {
            let sim = Sim::new();
            sim.state.borrow_mut().external_bus = true;
            sim
        });
        Self {
            actors,
            listeners,
            names,
            time: 0.0,
            stack: Vec::new(),
            pending: BTreeMap::new(),
            pending_seq: 0,
            variables,
            checkpoint: None,
            seed: options.seed,
            journal: Vec::new(),
            steps: 0,
            breakpoints: HashSet::new(),
            lua,
            chunks: input.chunks,
            options,
        }
    }

    pub fn actors(&self) -> &[Actor] {
        &self.actors
    }

    pub fn journal(&self) -> &[Entry] {
        &self.journal
    }

    /// Game time, seconds.
    pub fn time(&self) -> f64 {
        self.time
    }

    /// Steps done.
    pub fn steps(&self) -> u64 {
        self.steps
    }

    pub fn checkpoint(&self) -> Option<i32> {
        self.checkpoint
    }

    /// Variables: name (or hash), value, whether a dictionary or the run
    /// knows it.
    pub fn variables(&self) -> Vec<(String, i32, bool)> {
        self.variables
            .iter()
            .map(|(h, v)| (v.name.clone().unwrap_or_else(|| hex(*h)), v.value, v.known))
            .collect()
    }

    /// Hashes of the messages the scope's objects send or hear.
    pub fn scope_messages(&self) -> HashSet<u32> {
        self.names.keys().copied().collect()
    }

    /// The name of a message, when the scope writes it.
    pub fn message_name(&self, hash: u32) -> Option<&str> {
        self.names.get(&hash).map(String::as_str)
    }

    /// What waits in the queue: time and text, in order.
    pub fn pending(&self) -> Vec<(f64, String)> {
        let mut out: Vec<(f64, String)> = self
            .pending
            .values()
            .map(|(t, p)| {
                (
                    *t,
                    match p {
                        Pending::Send { from, msg, .. } => format!("{} → {}", from.label, msg.name),
                        Pending::Expire { actor, .. } => {
                            format!("{} expires", self.actors[*actor].label())
                        }
                        Pending::Input(i) => format!("input {}", i.text()),
                    },
                )
            })
            .collect();
        if let Some(t) = self.lua.as_ref().and_then(Sim::next_timer) {
            out.push((t, "Lua timer".to_owned()));
            out.sort_by(|a, b| a.0.total_cmp(&b.0));
        }
        out
    }

    /// Messages waiting to be delivered now (synchronous sends).
    pub fn waiting(&self) -> usize {
        self.stack.len()
    }

    /// Objects that match a selector: GUID (braced or 32 hex digits),
    /// `#<actor>`, `<stream>#<resource>:<object>`, or the name (`CMD_SetName`,
    /// letter case ignored).
    pub fn find(&self, selector: &str) -> Vec<usize> {
        let s = selector.trim();
        if let Some(n) = s.strip_prefix('#').and_then(|n| n.parse::<usize>().ok()) {
            return if n < self.actors.len() {
                vec![n]
            } else {
                Vec::new()
            };
        }
        if let Some(g) = crate::live::proto::parse_guid(s) {
            let g = Guid(g);
            return (0..self.actors.len())
                .filter(|&i| self.actors[i].guid == g)
                .collect();
        }
        if let Some((stream, rest)) = s.rsplit_once('#')
            && let Some((r, o)) = rest.split_once(':')
            && let (Ok(r), Ok(o)) = (r.parse::<usize>(), o.parse::<usize>())
        {
            return (0..self.actors.len())
                .filter(|&i| {
                    let a = &self.actors[i];
                    a.stream.eq_ignore_ascii_case(stream)
                        && a.location.resource == r
                        && a.object == o
                })
                .collect();
        }
        (0..self.actors.len())
            .filter(|&i| self.actors[i].name.eq_ignore_ascii_case(s))
            .collect()
    }

    /// The actor of a SimGroup object.
    pub fn actor_of(&self, location: Location, object: usize) -> Option<usize> {
        self.actors
            .iter()
            .position(|a| a.location == location && a.object == object)
    }

    /// Adds a manual reaction; returns the objects it was given to.
    pub fn add_reaction(&mut self, r: &ManualReaction) -> Vec<usize> {
        let found = self.find(&r.object);
        let send: Vec<Out> = r
            .send
            .iter()
            .filter(|o| !o.message.is_empty())
            .map(|o| Out {
                msg: Msg::new(&o.message),
                delay: o.delay.max(0.0),
            })
            .collect();
        for &i in &found {
            self.actors[i].manual.push(ManualSend {
                on: r.on.as_deref().filter(|s| !s.is_empty()).map(Msg::new),
                send: send.clone(),
            });
        }
        found
    }

    /// Removes the manual reactions of an object.
    pub fn clear_reactions(&mut self, actor: usize) {
        if let Some(a) = self.actors.get_mut(actor) {
            a.manual.clear();
        }
    }

    pub fn breakpoints(&self) -> &HashSet<Breakpoint> {
        &self.breakpoints
    }

    /// Switches a breakpoint on or off; returns whether it is on now.
    pub fn toggle_breakpoint(&mut self, b: Breakpoint) -> bool {
        if self.breakpoints.remove(&b) {
            false
        } else {
            self.breakpoints.insert(b);
            true
        }
    }

    /// Puts an input at game time `at` (now, if earlier).
    pub fn schedule(&mut self, at: f64, input: Input) {
        let at = at.max(self.time);
        self.push_pending(at, Pending::Input(input));
    }

    fn push_pending(&mut self, at: f64, p: Pending) {
        self.pending_seq += 1;
        self.pending
            .insert((time_key(at), self.pending_seq), (at, p));
    }

    fn log(&mut self, what: What) {
        let seq = self.journal.len() as u64 + 1;
        self.journal.push(Entry {
            seq,
            step: self.steps,
            time: self.time,
            what,
        });
    }

    fn breaks_before(&self, w: &Work) -> Option<String> {
        match w {
            Work::Send { msg, .. } if self.breakpoints.contains(&Breakpoint::Message(msg.hash)) => {
                Some(format!("message {}", msg.name))
            }
            Work::Deliver {
                to: Receiver::Actor(i),
                msg,
                ..
            } if self.breakpoints.contains(&Breakpoint::Object(*i)) => {
                Some(format!("{} gets {}", self.actors[*i].label(), msg.name))
            }
            _ => None,
        }
    }

    /// Game time of the next queued event, Lua timers included.
    fn next_due(&self) -> Option<(f64, bool)> {
        let queued = self.pending.values().next().map(|(t, _)| *t);
        let lua = self.lua.as_ref().and_then(Sim::next_timer);
        match (queued, lua) {
            (Some(q), Some(l)) if l < q => Some((l, true)),
            (Some(q), _) => Some((q, false)),
            (None, Some(l)) => Some((l.max(self.time), true)),
            (None, None) => None,
        }
    }

    /// One step: deliver one message (or send one), or move the time to the
    /// next queued event and handle it.
    pub fn step(&mut self) -> StepOutcome {
        self.step_inner(false, None)
    }

    fn step_inner(&mut self, check_break: bool, limit: Option<f64>) -> StepOutcome {
        if self.stack.is_empty() {
            let Some((due, lua)) = self.next_due() else {
                return StepOutcome::Idle;
            };
            if limit.is_some_and(|l| due > l) {
                return StepOutcome::Idle;
            }
            if lua {
                self.steps += 1;
                let dt = (due - self.time).max(0.0);
                let works = self.lua_call(0, |sim| sim.advance(dt).map(|()| None));
                self.time = due.max(self.time);
                self.push_works(works);
                return StepOutcome::Did;
            }
            let (&key, _) = self.pending.iter().next().expect("next_due saw it");
            let (t, p) = self.pending.remove(&key).expect("present");
            self.time = t.max(self.time);
            match p {
                Pending::Send { from, msg, target } => self.stack.push(Work::Send {
                    from,
                    msg,
                    target,
                    depth: 0,
                }),
                Pending::Expire { actor, generation } => {
                    self.steps += 1;
                    self.expire(actor, generation);
                    return StepOutcome::Did;
                }
                Pending::Input(input) => {
                    self.steps += 1;
                    self.apply_input(input);
                    return StepOutcome::Did;
                }
            }
        }
        let Some(w) = self.stack.last() else {
            return StepOutcome::Idle;
        };
        if check_break && self.breaks_before(w).is_some() {
            return StepOutcome::Break;
        }
        let w = self.stack.pop().expect("present");
        self.steps += 1;
        match w {
            Work::Send {
                from,
                msg,
                target,
                depth,
            } => self.send(from, msg, target, depth),
            Work::Deliver {
                to,
                msg,
                from,
                depth,
            } => self.deliver(to, msg, from, depth),
            Work::VariableChanged { hash, depth } => self.variable_changed(hash, depth),
        }
        StepOutcome::Did
    }

    /// Runs until `until`, a breakpoint or `max_steps` steps.
    pub fn run(&mut self, until: Until, max_steps: usize) -> Stop {
        let limit = match until {
            Until::Time(t) => Some(t),
            _ => None,
        };
        let mut done = 0usize;
        loop {
            if done >= max_steps {
                return Stop::Limit;
            }
            let before = self.journal.len();
            match self.step_inner(done > 0, limit) {
                StepOutcome::Idle => {
                    if let Some(t) = limit {
                        self.time = self.time.max(t);
                        return Stop::Time;
                    }
                    return Stop::Idle;
                }
                StepOutcome::Break => {
                    let what = self
                        .stack
                        .last()
                        .and_then(|w| self.breaks_before(w))
                        .unwrap_or_default();
                    return Stop::Breakpoint(what);
                }
                StepOutcome::Did => {}
            }
            done += 1;
            match until {
                Until::Steps(n) if done >= n => return Stop::Steps,
                Until::Message
                    if self.journal[before..]
                        .iter()
                        .any(|e| matches!(e.what, What::Send { .. })) =>
                {
                    return Stop::Message;
                }
                _ => {}
            }
        }
    }

    fn send(&mut self, from: Who, msg: Msg, target: Option<Guid>, depth: u32) {
        let mut to: Vec<Receiver> = self
            .listeners
            .get(&msg.hash)
            .into_iter()
            .flatten()
            .copied()
            .filter(|&i| target.is_none_or(|g| self.actors[i].guid == g))
            .map(Receiver::Actor)
            .collect();
        if target.is_none() && self.lua.as_ref().is_some_and(|l| l.listens(&msg.name)) {
            to.push(Receiver::Lua);
        }
        let engine = to.is_empty() && LogicSchema::gf2().engine_message(msg.hash).is_some();
        self.names
            .entry(msg.hash)
            .or_insert_with(|| msg.name.clone());
        let who: Vec<Who> = to
            .iter()
            .map(|r| match r {
                Receiver::Actor(i) => self.actors[*i].who(*i),
                Receiver::Lua => Who::lua(None),
            })
            .collect();
        self.log(What::Send {
            from: from.clone(),
            message: msg.name.clone(),
            hash: hex(msg.hash),
            to: who,
            target: target.map(|g| g.to_string()),
            engine,
            depth,
        });
        if depth >= self.options.max_depth {
            self.log(What::Warning {
                text: format!(
                    "{} sent {} levels deep without a delay: a loop (the game would overflow its stack); not delivered",
                    msg.name, depth
                ),
            });
            return;
        }
        for r in to.into_iter().rev() {
            self.stack.push(Work::Deliver {
                to: r,
                msg: msg.clone(),
                from: from.clone(),
                depth,
            });
        }
    }

    fn context_parts(&mut self) -> (f64, Option<i32>) {
        (self.time, self.checkpoint)
    }

    fn deliver(&mut self, to: Receiver, msg: Msg, from: Who, depth: u32) {
        let i = match to {
            Receiver::Lua => {
                let name = msg.name.clone();
                self.log(What::Deliver {
                    to: Who::lua(None),
                    message: msg.name.clone(),
                    from,
                    took: Took::Known,
                    reaction: "the scripts' receivers run".to_owned(),
                });
                let works = self.lua_call(depth, move |sim| sim.send(&name).map(|()| None));
                self.push_works(works);
                return;
            }
            Receiver::Actor(i) => i,
        };
        let (time, checkpoint) = self.context_parts();
        let vars = &self.variables;
        let seed = &mut self.seed;
        let lookup = |h: u32| vars.get(&h).map_or((0, false), |v| (v.value, v.known));
        let mut rand = || model::msvc_rand(seed);
        let mut ctx = Context {
            time,
            variable: &lookup,
            checkpoint,
            rand: &mut rand,
        };
        let actor = &mut self.actors[i];
        let mut reaction = actor.model.handle(msg.hash, &mut ctx);
        let mut took = if reaction.unknown {
            Took::Unknown
        } else {
            Took::Known
        };
        let manual: Vec<&Out> = actor
            .manual
            .iter()
            .filter(|m| m.on.as_ref().is_none_or(|o| o.hash == msg.hash))
            .flat_map(|m| &m.send)
            .collect();
        if !manual.is_empty() {
            let text = manual
                .iter()
                .map(|o| o.msg.name.clone())
                .collect::<Vec<_>>()
                .join(", ");
            if took == Took::Unknown {
                reaction.text = format!("manual: sends {text}");
                took = Took::Manual;
            } else {
                reaction.text.push_str(&format!("; manual: sends {text}"));
            }
            for o in manual {
                reaction.actions.push(Action::Send {
                    msg: o.msg.clone(),
                    delay: o.delay,
                    target: None,
                });
            }
        }
        let me = actor.who(i);
        self.log(What::Deliver {
            to: me.clone(),
            message: msg.name.clone(),
            from,
            took,
            reaction: reaction.text.clone(),
        });
        self.act(i, me, reaction.actions, depth);
    }

    /// Carries out the actions of a reaction of actor `i`: the synchronous
    /// ones go on the stack in order (the first is done first).
    fn act(&mut self, i: usize, me: Who, actions: Vec<Action>, depth: u32) {
        let mut works = Vec::new();
        for a in actions {
            match a {
                Action::Send { msg, delay, target } => {
                    if delay > 0.0 {
                        let due = self.time + delay;
                        self.log(What::Schedule {
                            from: me.clone(),
                            message: msg.name.clone(),
                            due,
                        });
                        self.push_pending(
                            due,
                            Pending::Send {
                                from: me.clone(),
                                msg,
                                target,
                            },
                        );
                    } else {
                        works.push(Work::Send {
                            from: me.clone(),
                            msg,
                            target,
                            depth: depth + 1,
                        });
                    }
                }
                Action::SetVariable { hash, value } => {
                    self.set_variable(hash, None, value, me.clone());
                    works.push(Work::VariableChanged {
                        hash,
                        depth: depth + 1,
                    });
                }
                Action::Timer { due, generation } => {
                    self.push_pending(
                        due,
                        Pending::Expire {
                            actor: i,
                            generation,
                        },
                    );
                }
                Action::LuaStart => works.extend(self.lua_start(i, depth)),
                Action::LuaStop => works.extend(self.lua_stop(i, depth)),
            }
        }
        self.push_works(works);
    }

    /// Puts synchronous work on the stack so that the first is done first.
    fn push_works(&mut self, works: Vec<Work>) {
        for w in works.into_iter().rev() {
            self.stack.push(w);
        }
    }

    fn set_variable(&mut self, hash: u32, name: Option<&str>, value: i32, by: Who) {
        let v = self.variables.entry(hash).or_insert(Variable {
            name: None,
            value: 0,
            known: false,
        });
        if v.name.is_none() {
            v.name = name.map(str::to_owned);
        }
        let old = v.value;
        v.value = value;
        v.known = true;
        let name = v.name.clone().unwrap_or_else(|| hex(hash));
        self.log(What::Variable {
            name,
            hash: hex(hash),
            old,
            value,
            by,
        });
    }

    fn variable_changed(&mut self, hash: u32, depth: u32) {
        let watchers: Vec<usize> = (0..self.actors.len())
            .filter(|&i| self.actors[i].model.watching() == Some(hash))
            .collect();
        let name = self
            .variables
            .get(&hash)
            .and_then(|v| v.name.clone())
            .unwrap_or_else(|| hex(hash));
        for i in watchers {
            let (time, checkpoint) = self.context_parts();
            let vars = &self.variables;
            let seed = &mut self.seed;
            let lookup = |h: u32| vars.get(&h).map_or((0, false), |v| (v.value, v.known));
            let mut rand = || model::msvc_rand(seed);
            let mut ctx = Context {
                time,
                variable: &lookup,
                checkpoint,
                rand: &mut rand,
            };
            let r = self.actors[i].model.check(&mut ctx);
            if r.text.is_empty() {
                continue;
            }
            let me = self.actors[i].who(i);
            self.log(What::Deliver {
                to: me.clone(),
                message: format!("({name} changed)"),
                from: Who::engine(),
                took: Took::Known,
                reaction: r.text,
            });
            self.act(i, me, r.actions, depth);
        }
    }

    fn expire(&mut self, actor: usize, generation: u32) {
        let Model::MissionTimer {
            state, on_expired, ..
        } = &mut self.actors[actor].model
        else {
            return;
        };
        match state {
            TimerState::Running { generation: g, .. } if *g == generation => {}
            _ => return,
        }
        *state = TimerState::Idle;
        let send = on_expired.clone();
        let me = self.actors[actor].who(actor);
        self.log(What::Deliver {
            to: me.clone(),
            message: "(timer)".to_owned(),
            from: Who::engine(),
            took: Took::Known,
            reaction: match &send {
                Some(m) => format!("expires: sends {}", m.name),
                None => "expires".to_owned(),
            },
        });
        if let Some(msg) = send {
            self.stack.push(Work::Send {
                from: me,
                msg,
                target: None,
                depth: 0,
            });
        }
    }

    fn apply_input(&mut self, input: Input) {
        self.log(What::Input { text: input.text() });
        match input {
            Input::Load => {
                let mut works = Vec::new();
                for i in 0..self.actors.len() {
                    if matches!(self.actors[i].model, Model::LuaScript { on_load: true, .. }) {
                        let me = self.actors[i].who(i);
                        self.log(What::Deliver {
                            to: me,
                            message: "(load)".to_owned(),
                            from: Who::engine(),
                            took: Took::Known,
                            reaction: "starts the script at load".to_owned(),
                        });
                        works.extend(self.lua_start(i, 0));
                    }
                }
                works.push(Work::Send {
                    from: Who::engine(),
                    msg: Msg::new(POST_SPAWN),
                    target: None,
                    depth: 0,
                });
                self.push_works(works);
            }
            Input::Message(m) => self.stack.push(Work::Send {
                from: Who::input(),
                msg: Msg::new(&m),
                target: None,
                depth: 0,
            }),
            Input::Enter(sel) => self.enter(&sel, true),
            Input::Exit(sel) => self.enter(&sel, false),
            Input::Variable(name, value) => {
                let h = string_id(&name);
                self.set_variable(h, Some(&name), value, Who::input());
                self.stack.push(Work::VariableChanged { hash: h, depth: 0 });
            }
            Input::Checkpoint(n) => self.checkpoint = Some(n),
        }
    }

    /// The player enters (or leaves) the trigger volumes `sel`.
    fn enter(&mut self, sel: &str, enter: bool) {
        let found: Vec<usize> = self
            .find(sel)
            .into_iter()
            .filter(|&i| matches!(self.actors[i].model, Model::Trigger { .. }))
            .collect();
        if found.is_empty() {
            self.log(What::Warning {
                text: format!("no trigger volume {sel:?} in the scope"),
            });
        }
        let mut works = Vec::new();
        for i in found {
            let Model::Trigger {
                on_enter,
                on_exit,
                on_leave,
                remaining,
                switch,
                ..
            } = &mut self.actors[i].model
            else {
                continue;
            };
            // A touch sensor with `on_leave` sends its message when the
            // touch ends; an enter/exit trigger has a message for each.
            let out = match (enter, *on_leave) {
                (true, false) | (false, true) => on_enter.clone(),
                (true, true) => None,
                (false, false) => on_exit.clone().map(|msg| Out { msg, delay: 0.0 }),
            };
            // The send count of a touch sensor: at 0 the object asks to be
            // removed (`0x004C1260`).
            let mut used_up = false;
            if out.is_some() && *switch != Switch::Off && *remaining > 0 {
                *remaining -= 1;
                if *remaining == 0 {
                    *switch = Switch::Off;
                    used_up = true;
                }
            }
            let switch = if used_up { Switch::On } else { *switch };
            let me = self.actors[i].who(i);
            let what = if enter {
                "the player enters"
            } else {
                "the player leaves"
            };
            if switch == Switch::Off {
                self.log(What::Deliver {
                    to: me,
                    message: format!("({what})"),
                    from: Who::input(),
                    took: Took::Known,
                    reaction: "switched off: nothing".to_owned(),
                });
                continue;
            }
            let note = if switch == Switch::Unknown {
                " (switch state not known: taken as on)"
            } else {
                ""
            };
            self.log(What::Deliver {
                to: me.clone(),
                message: format!("({what})"),
                from: Who::input(),
                took: Took::Known,
                reaction: match &out {
                    Some(o) if used_up => {
                        format!("sends {}; used up (max_fires reached)", o.msg.name)
                    }
                    Some(o) => format!("sends {}{note}", o.msg.name),
                    None => format!("nothing to send{note}"),
                },
            });
            if let Some(o) = out {
                if o.delay > 0.0 {
                    let due = self.time + o.delay;
                    self.log(What::Schedule {
                        from: me.clone(),
                        message: o.msg.name.clone(),
                        due,
                    });
                    self.push_pending(
                        due,
                        Pending::Send {
                            from: me,
                            msg: o.msg,
                            target: None,
                        },
                    );
                } else {
                    works.push(Work::Send {
                        from: me,
                        msg: o.msg,
                        target: None,
                        depth: 0,
                    });
                }
            }
        }
        self.push_works(works);
    }

    // --- Lua ---------------------------------------------------------------

    /// Calls into the scripts and turns what they did into journal entries
    /// and work: messages they sent go out after the call returns (in the
    /// game `SendMessage` is synchronous: see `docs/logic-graph.md` §9.6).
    fn lua_call<F>(&mut self, depth: u32, f: F) -> Vec<Work>
    where
        F: FnOnce(&mut Sim) -> Result<Option<String>>,
    {
        let Some(mut sim) = self.lua.take() else {
            return Vec::new();
        };
        sim.state.borrow_mut().time = self.time;
        sim.vm.steps = 0;
        for v in self.variables.values() {
            if let Some(n) = &v.name {
                sim.set_variable(n, LuaValue::Number(f64::from(v.value)));
            }
        }
        let result = f(&mut sim);
        let log = sim.take_log();
        let printed: Vec<String> = std::mem::take(&mut sim.vm.printed);
        self.lua = Some(sim);
        let mut works = Vec::new();
        let lua = Who::lua(None);
        for e in log {
            match e {
                SimEvent::Message { message, .. } => {
                    works.push(Work::Send {
                        from: lua.clone(),
                        msg: Msg::new(&message),
                        target: None,
                        depth: depth + 1,
                    });
                }
                SimEvent::Variable { name, value, .. } => {
                    let v = value.parse::<f64>().map_or(0, |x| x as i32);
                    let h = string_id(&name);
                    let changed = self.variables.get(&h).is_none_or(|x| x.value != v);
                    if changed {
                        self.set_variable(h, Some(&name), v, lua.clone());
                        works.push(Work::VariableChanged {
                            hash: h,
                            depth: depth + 1,
                        });
                    }
                }
                SimEvent::Probe { file, .. } => self.log(What::Lua {
                    text: format!("loadfile({file:?})"),
                }),
                SimEvent::Call { function, args, .. } => self.log(What::Lua {
                    text: format!("{function}({})", args.join(", ")),
                }),
                SimEvent::Timer { callback, .. } => self.log(What::Lua {
                    text: format!("timer {callback}"),
                }),
                SimEvent::Error { message, .. } => self.log(What::Lua {
                    text: format!("error: {message}"),
                }),
            }
        }
        for p in printed {
            self.log(What::Lua {
                text: format!("print: {p}"),
            });
        }
        match result {
            Ok(Some(text)) => self.log(What::Lua { text }),
            Ok(None) => {}
            Err(e) => self.log(What::Lua {
                text: format!("error (the game swallows it): {e}"),
            }),
        }
        works
    }

    fn lua_start(&mut self, i: usize, depth: u32) -> Vec<Work> {
        if self.lua.is_none() {
            self.log(What::Warning {
                text: "Lua is off in this run: the script does not start".to_owned(),
            });
            return Vec::new();
        }
        let Model::LuaScript {
            resource, script, ..
        } = &self.actors[i].model
        else {
            return Vec::new();
        };
        let (resource, script) = (*resource, script.clone());
        if let Some(name) = script {
            return self.lua_call(depth, move |sim| sim.start(&name).map(|()| None));
        }
        let Some(chunk) = resource.and_then(|id| self.chunks.get(&id).cloned()) else {
            self.log(What::Warning {
                text: format!(
                    "{}: lua resource {} not found — in the game the script silently does not run",
                    self.actors[i].label(),
                    resource.map_or_else(|| "(none)".to_owned(), hex)
                ),
            });
            return Vec::new();
        };
        let before: HashSet<String> = self
            .lua
            .as_ref()
            .map(|s| s.scripts().into_iter().collect())
            .unwrap_or_default();
        let mut works = self.lua_call(depth, |sim| sim.load(&chunk).map(|()| None));
        let new: Vec<String> = self
            .lua
            .as_ref()
            .map(|s| {
                s.scripts()
                    .into_iter()
                    .filter(|n| !before.contains(n))
                    .collect()
            })
            .unwrap_or_default();
        if let Some(name) = new.first().cloned() {
            if let Model::LuaScript { script, .. } = &mut self.actors[i].model {
                *script = Some(name.clone());
            }
            self.log(What::Lua {
                text: format!("{name} loaded, startup"),
            });
            let registered = self
                .lua
                .as_ref()
                .is_some_and(|s| s.registered(&name, "startup"));
            if registered {
                works.extend(self.lua_call(depth, move |sim| sim.start(&name).map(|()| None)));
            }
        } else {
            self.log(What::Lua {
                text: "the chunk registered no functions (a library: nothing to start)".to_owned(),
            });
        }
        works
    }

    fn lua_stop(&mut self, i: usize, depth: u32) -> Vec<Work> {
        let Model::LuaScript {
            script: Some(name), ..
        } = &self.actors[i].model
        else {
            return Vec::new();
        };
        let name = name.clone();
        if !self
            .lua
            .as_ref()
            .is_some_and(|s| s.registered(&name, "shutdown"))
        {
            return Vec::new();
        }
        self.lua_call(depth, move |sim| sim.stop(&name).map(|()| None))
    }
}

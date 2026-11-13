//! What a class does with a message: models of the classes whose handler
//! was read in `godfather2.exe` (`docs/logic-graph.md` §9.2). Everything
//! else is a black box: the message arrives, the reaction is not known.

use serde::Serialize;

use crate::hash::string_id;
use crate::resource::simgroup::{Guid, Object, SimGroup, Value};

use super::super::{LogicSchema, PinKind};

/// Packet classes of the modelled classes (`data/gf2/logic_schema.toml`).
pub(super) mod class {
    pub const MULTI_MANAGER: u32 = 0x6df5_0074;
    pub const MESSAGE_RELAY: u32 = 0x120f_25aa;
    pub const EVENT_GATE: u32 = 0x7be1_94ee;
    pub const OR_EVENT_GATE: u32 = 0x9dc6_6782;
    pub const TRIGGER_AUTO: u32 = 0xacbd_fe47;
    pub const TRIGGER_RANDOM: u32 = 0x62eb_e09b;
    pub const VARIABLE_OPERATOR: u32 = 0x5ee8_ce40;
    pub const VARIABLE_SWITCH: u32 = 0x5555_e170;
    pub const VARIABLE_WATCHER: u32 = 0x2915_5ec0;
    pub const MISSION_TIMER: u32 = 0x805c_da59;
    pub const TEST_CURRENT_CHECKPOINT: u32 = 0xa863_1a6f;
    pub const TRIGGER_BASE: u32 = 0xd16a_98a9;
    pub const ENTER_EXIT_TRIGGER: u32 = 0xc8c5_d222;
    pub const LUA_AUDIO_SCRIPT: u32 = 0x07b6_c269;
}

/// The message the game sends to the objects of a SimGroup once they are
/// all made (`0x0110064C`, set in `0x00445530`).
pub const POST_SPAWN: &str = "iMsgPostSpawnInitialize";

/// A message name and its hash.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Msg {
    pub name: String,
    pub hash: u32,
}

impl Msg {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_owned(),
            hash: string_id(name),
        }
    }
}

/// A message an object sends, with its delay in seconds.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Out {
    pub msg: Msg,
    pub delay: f64,
}

/// State of a trigger volume: whether it is switched on, when known.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Switch {
    Unknown,
    On,
    Off,
}

/// State of a `MissionTimer`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum TimerState {
    Idle,
    Running { due: f64, generation: u32 },
    Paused { left: f64 },
}

/// A class model and its state.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "model", rename_all = "snake_case")]
pub enum Model {
    /// `0x0047A1B0`: on `trigger` sends every pair (delay 0 — at once, in
    /// order), or with flag 1 one pair per trigger in turn; `max_fires`
    /// counts down (−1 — forever).
    MultiManager {
        trigger: Option<Msg>,
        sends: Vec<Out>,
        remaining: i32,
        sequential: bool,
        next: usize,
    },
    /// `0x004CF8D0` → `0x004CF950`: on `trigger` sends `send` at once; with
    /// a target GUID (attribute 3) only that object hears it.
    MessageRelay {
        trigger: Option<Msg>,
        send: Option<Msg>,
        target: Option<Guid>,
    },
    /// `0x00479B50`: every set condition must arrive (in any order), then
    /// `send` goes out and the gate starts again; `reset` forgets them.
    AndGate {
        conditions: Vec<Option<Msg>>,
        send: Option<Out>,
        reset: Option<Msg>,
        got: u8,
    },
    /// `0x00479E20`: any set condition sends `send`; afterwards the gate
    /// ignores conditions for `lockout` seconds; `reset` ends the lockout.
    OrGate {
        conditions: Vec<Option<Msg>>,
        send: Option<Out>,
        reset: Option<Msg>,
        lockout: f64,
        locked_until: f64,
    },
    /// `0x00477EE0`: once, on [`POST_SPAWN`], sends `send` after its delay.
    TriggerAuto { send: Option<Out>, fired: bool },
    /// `0x0047AF50`: on `trigger` picks one pair by weight (`rand()`).
    TriggerRandom {
        trigger: Option<Msg>,
        choices: Vec<(f64, Out)>,
    },
    /// `0x00476970` → `0x004768C0`: on `trigger` changes the variable:
    /// 0 add, 1 subtract, 2 set, 3 switch between 0 and 1.
    VariableOperator {
        trigger: Option<Msg>,
        variable: Option<u32>,
        operation: i32,
        operand: i32,
    },
    /// `0x00476E40`: on `trigger` sends every case whose value equals the
    /// variable; when none does, `default`.
    VariableSwitch {
        trigger: Option<Msg>,
        variable: Option<u32>,
        default: Option<Out>,
        cases: Vec<(i32, Out)>,
    },
    /// `0x00477A80`, `0x00477BD0`: once active (`activate`, or
    /// [`POST_SPAWN`] with flag 1), checks the variable now and on every
    /// change: bit 2 equal, bit 1 less, bit 4 greater than `value`; sends
    /// `send` and stops watching, unless flag 4 keeps it watching.
    VariableWatcher {
        activate: Option<Msg>,
        deactivate: Option<Msg>,
        on_spawn: bool,
        send: Option<Out>,
        variable: Option<u32>,
        comparison: i32,
        value: i32,
        keep: bool,
        max_fires: i32,
        fired: i32,
        active: bool,
    },
    /// `0x0090DF30`, reader `0x0090DA30`: `start` runs it for
    /// `seconds − start_at`, then `on_expired`; `stop` cancels it and sends
    /// `on_stopped`; `pause`/`resume`.
    MissionTimer {
        start: Option<Msg>,
        stop: Option<Msg>,
        pause: Option<Msg>,
        resume: Option<Msg>,
        duration: f64,
        on_expired: Option<Msg>,
        on_stopped: Option<Msg>,
        state: TimerState,
        /// Starts so far: a queued end of an earlier start is stale.
        starts: u32,
    },
    /// `0x008FF4F0`: on `test` compares the current checkpoint with
    /// `checkpoint` (0 `<`, 1 `<=`, 2 `==`, 3 `>=`, 4 `>`; current on the
    /// right for 0–1) and sends `on_true` or `on_false`.
    TestCheckpoint {
        test: Option<Msg>,
        comparison: i32,
        checkpoint: i32,
        on_true: Option<Msg>,
        on_false: Option<Msg>,
    },
    /// Trigger volumes (`TriggerBase` `0x004C1260`, `EnterExitTrigger`
    /// `0x00900B30`): switched on and off by messages; the player entering
    /// or leaving is an input of the run.
    Trigger {
        toggle: Option<Msg>,
        enable: Option<Msg>,
        disable: Option<Msg>,
        on_enter: Option<Out>,
        on_exit: Option<Msg>,
        switch: Switch,
    },
    /// `LuaAudioScript` (`docs/formats/mission-logic.md` §3.2): runs a Lua
    /// chunk on `start` (or at load with flag 1), `shutdown` on `stop`.
    LuaScript {
        start: Option<Msg>,
        stop: Option<Msg>,
        on_load: bool,
        resource: Option<u32>,
        script: Option<String>,
    },
    /// Not modelled: messages arrive, the reaction is not known.
    BlackBox,
}

impl Model {
    /// Name of the model, `black_box` for unknown classes.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::MultiManager { .. } => "multi_manager",
            Self::MessageRelay { .. } => "message_relay",
            Self::AndGate { .. } => "and_gate",
            Self::OrGate { .. } => "or_gate",
            Self::TriggerAuto { .. } => "trigger_auto",
            Self::TriggerRandom { .. } => "trigger_random",
            Self::VariableOperator { .. } => "variable_operator",
            Self::VariableSwitch { .. } => "variable_switch",
            Self::VariableWatcher { .. } => "variable_watcher",
            Self::MissionTimer { .. } => "mission_timer",
            Self::TestCheckpoint { .. } => "test_checkpoint",
            Self::Trigger { .. } => "trigger",
            Self::LuaScript { .. } => "lua_script",
            Self::BlackBox => "black_box",
        }
    }

    pub fn is_black_box(&self) -> bool {
        matches!(self, Self::BlackBox)
    }

    /// Messages the model reacts to besides the `in` pins of the schema.
    pub fn implicit(&self) -> Vec<Msg> {
        match self {
            Self::TriggerAuto { .. } => vec![Msg::new(POST_SPAWN)],
            Self::VariableWatcher { on_spawn: true, .. } => vec![Msg::new(POST_SPAWN)],
            _ => Vec::new(),
        }
    }
}

/// Attribute reader of one object.
pub(super) struct Attrs<'a> {
    pub group: &'a SimGroup,
    pub object: &'a Object,
}

impl Attrs<'_> {
    fn value(&self, class: u32, index: u16) -> Option<&Value> {
        self.group.attr(self.object, class, index)
    }

    pub fn has(&self, class: u32) -> bool {
        self.group
            .object_packets(self.object)
            .any(|p| p.class.0 == class)
    }

    pub fn msg(&self, class: u32, index: u16) -> Option<Msg> {
        match self.value(class, index) {
            Some(Value::String(s)) if !s.is_empty() => Some(Msg::new(s)),
            _ => None,
        }
    }

    pub fn float(&self, class: u32, index: u16) -> Option<f64> {
        match self.value(class, index)? {
            // The shortest decimal of the f32: 0.1, not 0.10000000149.
            Value::Float(f) => f.to_string().parse().ok(),
            Value::Int(i) => Some(f64::from(*i)),
            _ => None,
        }
    }

    pub fn int(&self, class: u32, index: u16) -> Option<i32> {
        match self.value(class, index)? {
            Value::Int(i) => Some(*i),
            Value::Float(f) => Some(*f as i32),
            Value::Hash(h) => Some(h.0 as i32),
            _ => None,
        }
    }

    /// A variable named by hash (or by name).
    pub fn variable(&self, class: u32, index: u16) -> Option<u32> {
        match self.value(class, index)? {
            Value::Hash(h) if h.0 != 0 => Some(h.0),
            Value::Int(i) if *i != 0 => Some(*i as u32),
            Value::String(s) if !s.is_empty() => Some(string_id(s)),
            _ => None,
        }
    }

    pub fn guid(&self, class: u32, index: u16) -> Option<Guid> {
        match self.value(class, index)? {
            Value::Guid(g) if !g.is_nil() => Some(*g),
            _ => None,
        }
    }

    fn out(&self, class: u32, msg: u16, delay: Option<u16>) -> Option<Out> {
        Some(Out {
            msg: self.msg(class, msg)?,
            delay: delay.and_then(|d| self.float(class, d)).unwrap_or(0.0),
        })
    }
}

/// The model of an object, from its class name and packets.
pub(super) fn build(class_name: &str, a: &Attrs<'_>) -> Model {
    use class::*;
    match class_name {
        "MultiManager" if a.has(MULTI_MANAGER) => Model::MultiManager {
            trigger: a.msg(MULTI_MANAGER, 0),
            sends: (0..8)
                .filter_map(|k| a.out(MULTI_MANAGER, 3 + 2 * k, Some(4 + 2 * k)))
                .collect(),
            // Attribute 1 is set in every object of the game (−1 in most);
            // −1 when missing.
            remaining: a.int(MULTI_MANAGER, 1).unwrap_or(-1),
            sequential: a.int(MULTI_MANAGER, 2).unwrap_or(0) & 1 != 0,
            next: 0,
        },
        "MessageRelay" if a.has(MESSAGE_RELAY) => {
            // Attributes 2 (a hash) and 4 (a GUID) change whom the relay
            // sends to in ways not read: such relays stay black boxes.
            let other = a.int(MESSAGE_RELAY, 2).is_some_and(|h| h != -1 && h != 0)
                || a.guid(MESSAGE_RELAY, 4).is_some();
            if other {
                Model::BlackBox
            } else {
                Model::MessageRelay {
                    trigger: a.msg(MESSAGE_RELAY, 0),
                    send: a.msg(MESSAGE_RELAY, 1),
                    target: a.guid(MESSAGE_RELAY, 3),
                }
            }
        }
        "AndEventGate" if a.has(EVENT_GATE) => Model::AndGate {
            conditions: (0..4).map(|i| a.msg(EVENT_GATE, i)).collect(),
            send: a.out(EVENT_GATE, 4, Some(5)),
            reset: a.msg(EVENT_GATE, 6),
            got: 0,
        },
        "OrEventGate" if a.has(EVENT_GATE) => Model::OrGate {
            conditions: (0..4).map(|i| a.msg(EVENT_GATE, i)).collect(),
            send: a.out(EVENT_GATE, 4, Some(5)),
            reset: a.msg(EVENT_GATE, 6),
            lockout: a.float(OR_EVENT_GATE, 0).unwrap_or(0.0),
            locked_until: f64::NEG_INFINITY,
        },
        "TriggerAuto" if a.has(TRIGGER_AUTO) => Model::TriggerAuto {
            send: a.out(TRIGGER_AUTO, 0, Some(1)),
            fired: false,
        },
        "TriggerRandom" if a.has(TRIGGER_RANDOM) => Model::TriggerRandom {
            trigger: a.msg(TRIGGER_RANDOM, 0),
            choices: (0..8)
                .filter_map(|k| {
                    let out = a.out(TRIGGER_RANDOM, 2 + 3 * k, Some(3 + 3 * k))?;
                    Some((a.float(TRIGGER_RANDOM, 1 + 3 * k).unwrap_or(0.0), out))
                })
                .collect(),
        },
        "VariableOperator" if a.has(VARIABLE_OPERATOR) => Model::VariableOperator {
            trigger: a.msg(VARIABLE_OPERATOR, 0),
            variable: a.variable(VARIABLE_OPERATOR, 2),
            operation: a.int(VARIABLE_OPERATOR, 3).unwrap_or(0),
            operand: a.int(VARIABLE_OPERATOR, 4).unwrap_or(0),
        },
        "VariableSwitch" if a.has(VARIABLE_SWITCH) => Model::VariableSwitch {
            trigger: a.msg(VARIABLE_SWITCH, 0),
            variable: a
                .variable(VARIABLE_SWITCH, 1)
                .or_else(|| a.variable(VARIABLE_SWITCH, 2)),
            default: a.out(VARIABLE_SWITCH, 3, Some(4)),
            cases: (0..8)
                .filter_map(|k| {
                    let out = a.out(VARIABLE_SWITCH, 6 + 3 * k, Some(7 + 3 * k))?;
                    Some((a.int(VARIABLE_SWITCH, 5 + 3 * k).unwrap_or(0), out))
                })
                .collect(),
        },
        "VariableWatcher" if a.has(VARIABLE_WATCHER) => {
            let flags = a.int(VARIABLE_WATCHER, 7).unwrap_or(0);
            Model::VariableWatcher {
                activate: a.msg(VARIABLE_WATCHER, 0),
                deactivate: a.msg(VARIABLE_WATCHER, 1),
                on_spawn: flags & 1 != 0,
                send: a.out(VARIABLE_WATCHER, 2, Some(3)),
                variable: a.variable(VARIABLE_WATCHER, 4),
                comparison: a.int(VARIABLE_WATCHER, 5).unwrap_or(0),
                value: a.int(VARIABLE_WATCHER, 6).unwrap_or(0),
                keep: flags & 4 != 0,
                max_fires: a.int(VARIABLE_WATCHER, 8).unwrap_or(0),
                fired: 0,
                active: false,
            }
        }
        "MissionTimer" if a.has(MISSION_TIMER) => {
            let seconds = a.float(MISSION_TIMER, 3).unwrap_or(0.0);
            let start_at = a.float(MISSION_TIMER, 9).unwrap_or(0.0).min(seconds);
            Model::MissionTimer {
                start: a.msg(MISSION_TIMER, 0),
                stop: a.msg(MISSION_TIMER, 1),
                pause: a.msg(MISSION_TIMER, 10),
                resume: a.msg(MISSION_TIMER, 11),
                duration: (seconds - start_at).max(0.0),
                on_expired: a.msg(MISSION_TIMER, 7),
                on_stopped: a.msg(MISSION_TIMER, 8),
                state: TimerState::Idle,
                starts: 0,
            }
        }
        "TestCurrentCheckpoint" if a.has(TEST_CURRENT_CHECKPOINT) => Model::TestCheckpoint {
            test: a.msg(TEST_CURRENT_CHECKPOINT, 2),
            comparison: a.int(TEST_CURRENT_CHECKPOINT, 0).unwrap_or(2),
            checkpoint: a.int(TEST_CURRENT_CHECKPOINT, 1).unwrap_or(0),
            on_true: a.msg(TEST_CURRENT_CHECKPOINT, 3),
            on_false: a.msg(TEST_CURRENT_CHECKPOINT, 4),
        },
        "EnterExitTrigger" if a.has(ENTER_EXIT_TRIGGER) => Model::Trigger {
            toggle: None,
            enable: a.msg(ENTER_EXIT_TRIGGER, 4),
            disable: a.msg(ENTER_EXIT_TRIGGER, 5),
            on_enter: a.out(ENTER_EXIT_TRIGGER, 0, None),
            on_exit: a.msg(ENTER_EXIT_TRIGGER, 1),
            switch: Switch::Unknown,
        },
        "LuaAudioScript" if a.has(LUA_AUDIO_SCRIPT) => Model::LuaScript {
            start: a.msg(LUA_AUDIO_SCRIPT, 1),
            stop: a.msg(LUA_AUDIO_SCRIPT, 2),
            on_load: a.int(LUA_AUDIO_SCRIPT, 0).unwrap_or(0) & 1 != 0,
            resource: a.guid(LUA_AUDIO_SCRIPT, 3).and_then(|g| g.resource_id()),
            script: None,
        },
        _ if a.has(TRIGGER_BASE) => Model::Trigger {
            toggle: a.msg(TRIGGER_BASE, 0),
            enable: a.msg(TRIGGER_BASE, 1),
            disable: a.msg(TRIGGER_BASE, 2),
            on_enter: a.out(TRIGGER_BASE, 4, Some(5)),
            on_exit: a.msg(TRIGGER_BASE, 7),
            switch: Switch::Unknown,
        },
        _ => Model::BlackBox,
    }
}

/// Messages an object listens to by the schema (`in` pins with a value),
/// with the pin names.
pub(super) fn listened(
    schema: &LogicSchema,
    group: &SimGroup,
    object: &Object,
) -> Vec<(Msg, String)> {
    let mut out: Vec<(Msg, String)> = Vec::new();
    for op in schema.object_pins(group, object) {
        if op.attr.pin != PinKind::In {
            continue;
        }
        if let Some(Value::String(s)) = &op.value
            && !s.is_empty()
        {
            let name = op
                .attr
                .name
                .clone()
                .unwrap_or_else(|| format!("{:08x}.{}", op.class, op.attr.index));
            let m = Msg::new(s);
            if !out.iter().any(|(x, _)| x.hash == m.hash) {
                out.push((m, name));
            }
        }
    }
    out
}

/// What [`Model::handle`] tells the run to do.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Action {
    /// Send a message: at once (delay 0) or later; to one object only when
    /// `target` is set.
    Send {
        msg: Msg,
        delay: f64,
        target: Option<Guid>,
    },
    /// Set a game variable.
    SetVariable {
        hash: u32,
        value: i32,
    },
    /// Run a `MissionTimer` to its end.
    Timer {
        due: f64,
        generation: u32,
    },
    /// Start or stop a `LuaAudioScript`.
    LuaStart,
    LuaStop,
}

/// What the run knows when a model reacts.
pub(super) struct Context<'a> {
    pub time: f64,
    pub variable: &'a dyn Fn(u32) -> (i32, bool),
    pub checkpoint: Option<i32>,
    pub rand: &'a mut dyn FnMut() -> u32,
}

/// The reaction of a model to a message.
#[derive(Debug, Clone, PartialEq, Default)]
pub(super) struct Reaction {
    /// What happened, in a few words.
    pub text: String,
    pub actions: Vec<Action>,
    /// The model has no reaction for this message (or it is a black box).
    pub unknown: bool,
}

impl Reaction {
    fn said(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            ..Self::default()
        }
    }

    fn send(&mut self, out: &Out) {
        self.actions.push(Action::Send {
            msg: out.msg.clone(),
            delay: out.delay,
            target: None,
        });
    }
}

fn is(m: &Option<Msg>, hash: u32) -> bool {
    m.as_ref().is_some_and(|m| m.hash == hash)
}

fn sends_text(outs: &[&Out]) -> String {
    outs.iter()
        .map(|o| {
            if o.delay > 0.0 {
                format!("{} (+{} s)", o.msg.name, o.delay)
            } else {
                o.msg.name.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// MSVC `rand()` on a seed.
pub(super) fn msvc_rand(seed: &mut u32) -> u32 {
    *seed = seed.wrapping_mul(214_013).wrapping_add(2_531_011);
    (*seed >> 16) & 0x7fff
}

impl Model {
    /// Reacts to message `hash`.
    #[allow(clippy::too_many_lines)]
    pub(super) fn handle(&mut self, hash: u32, ctx: &mut Context<'_>) -> Reaction {
        let post_spawn = string_id(POST_SPAWN);
        match self {
            Self::MultiManager {
                trigger,
                sends,
                remaining,
                sequential,
                next,
            } => {
                if !is(trigger, hash) {
                    return Reaction::said("heard; no reaction in the handler");
                }
                if *remaining == 0 {
                    return Reaction::said("used up (max_fires reached)");
                }
                if sends.is_empty() {
                    return Reaction::said("nothing to send");
                }
                if *remaining > 0 {
                    *remaining -= 1;
                }
                let mut r = Reaction::default();
                if *sequential {
                    let out = sends[*next % sends.len()].clone();
                    *next = (*next + 1) % sends.len();
                    r.text = format!("sends in turn: {}", sends_text(&[&out]));
                    r.send(&out);
                } else {
                    r.text = format!("sends {}", sends_text(&sends.iter().collect::<Vec<_>>()));
                    for out in sends.iter() {
                        r.send(out);
                    }
                }
                if *remaining == 0 {
                    r.text.push_str("; used up");
                }
                r
            }
            Self::MessageRelay {
                trigger,
                send,
                target,
            } => {
                if !is(trigger, hash) {
                    return Reaction::said("heard; no reaction in the handler");
                }
                let Some(send) = send else {
                    return Reaction::said("nothing to send");
                };
                Reaction {
                    text: match target {
                        Some(g) => format!("relays {} to {g}", send.name),
                        None => format!("relays {}", send.name),
                    },
                    actions: vec![Action::Send {
                        msg: send.clone(),
                        delay: 0.0,
                        target: *target,
                    }],
                    unknown: false,
                }
            }
            Self::AndGate {
                conditions,
                send,
                reset,
                got,
            } => {
                if is(reset, hash) {
                    *got = 0;
                    return Reaction::said("reset");
                }
                let need: u8 = conditions
                    .iter()
                    .enumerate()
                    .filter(|(_, c)| c.is_some())
                    .fold(0, |m, (i, _)| m | (1 << i));
                let Some(i) = conditions.iter().position(|c| is(c, hash)) else {
                    return Reaction::said("heard; no reaction in the handler");
                };
                *got |= 1 << i;
                if *got != need {
                    return Reaction::said(format!(
                        "condition {} of {}",
                        got.count_ones(),
                        need.count_ones()
                    ));
                }
                *got = 0;
                match send {
                    Some(out) => {
                        let mut r =
                            Reaction::said(format!("all conditions: sends {}", sends_text(&[out])));
                        r.send(out);
                        r
                    }
                    None => Reaction::said("all conditions; nothing to send"),
                }
            }
            Self::OrGate {
                conditions,
                send,
                reset,
                lockout,
                locked_until,
            } => {
                let mut r = Reaction::said("heard; no reaction in the handler");
                if ctx.time >= *locked_until && conditions.iter().any(|c| is(c, hash)) {
                    match send {
                        Some(out) => {
                            r = Reaction::said(format!("sends {}", sends_text(&[out])));
                            r.send(out);
                        }
                        None => r = Reaction::said("nothing to send"),
                    }
                    *locked_until = ctx.time + *lockout;
                } else if conditions.iter().any(|c| is(c, hash)) {
                    r = Reaction::said(format!("locked until {:.3} s", *locked_until));
                }
                if is(reset, hash) {
                    *locked_until = f64::NEG_INFINITY;
                    r.text = "reset".to_owned();
                }
                r
            }
            Self::TriggerAuto { send, fired } => {
                if hash != post_spawn || *fired {
                    return Reaction::said("heard; no reaction in the handler");
                }
                *fired = true;
                match send {
                    Some(out) => {
                        let mut r = Reaction::said(format!("fires once: {}", sends_text(&[out])));
                        r.send(out);
                        r
                    }
                    None => Reaction::said("fires once; nothing to send"),
                }
            }
            Self::TriggerRandom { trigger, choices } => {
                if !is(trigger, hash) {
                    return Reaction::said("heard; no reaction in the handler");
                }
                let total: f64 = choices.iter().map(|(w, _)| *w).sum();
                let roll = f64::from((ctx.rand)()) / 32767.0 * total;
                let mut sum = 0.0;
                for (w, out) in choices.iter() {
                    sum += *w;
                    if sum > roll {
                        let mut r = Reaction::said(format!("picks {}", sends_text(&[out])));
                        r.send(out);
                        return r;
                    }
                }
                Reaction::said("picks nothing")
            }
            Self::VariableOperator {
                trigger,
                variable,
                operation,
                operand,
            } => {
                if !is(trigger, hash) {
                    return Reaction::said("heard; no reaction in the handler");
                }
                let Some(v) = *variable else {
                    return Reaction::said("no variable");
                };
                let (old, known) = (ctx.variable)(v);
                let new = match *operation {
                    0 => old.wrapping_add(*operand),
                    1 => old.wrapping_sub(*operand),
                    2 => *operand,
                    3 => i32::from(old == 0),
                    op => return Reaction::said(format!("operation {op} is not known")),
                };
                let mut r = Reaction::said(format!(
                    "{} {old} → {new}{}",
                    ["add", "subtract", "set", "switch"][*operation as usize],
                    if known {
                        ""
                    } else {
                        " (variable not in the dictionaries: 0 assumed)"
                    }
                ));
                r.actions.push(Action::SetVariable {
                    hash: v,
                    value: new,
                });
                r
            }
            Self::VariableSwitch {
                trigger,
                variable,
                default,
                cases,
            } => {
                if !is(trigger, hash) {
                    return Reaction::said("heard; no reaction in the handler");
                }
                let Some(v) = *variable else {
                    return Reaction::said("no variable");
                };
                let (value, known) = (ctx.variable)(v);
                let hits: Vec<&Out> = cases
                    .iter()
                    .filter(|(x, _)| *x == value)
                    .map(|(_, o)| o)
                    .collect();
                let note = if known {
                    ""
                } else {
                    " (variable not in the dictionaries: 0 assumed)"
                };
                let mut r = Reaction::default();
                if hits.is_empty() {
                    match default {
                        Some(out) => {
                            r.text = format!("value {value}{note}: default {}", sends_text(&[out]));
                            r.send(out);
                        }
                        None => r.text = format!("value {value}{note}: no case, no default"),
                    }
                } else {
                    r.text = format!("value {value}{note}: {}", sends_text(&hits));
                    for out in hits {
                        r.send(out);
                    }
                }
                r
            }
            Self::VariableWatcher {
                activate,
                deactivate,
                on_spawn,
                active,
                ..
            } => {
                if is(deactivate, hash) {
                    *active = false;
                    return Reaction::said("stops watching");
                }
                if is(activate, hash) || (*on_spawn && hash == post_spawn) {
                    *active = true;
                    let mut r = self.check(ctx);
                    if r.text.is_empty() {
                        r.text = "watches the variable".to_owned();
                    }
                    return r;
                }
                Reaction::said("heard; no reaction in the handler")
            }
            Self::MissionTimer {
                start,
                stop,
                pause,
                resume,
                duration,
                on_expired: _,
                on_stopped,
                state,
                starts,
            } => {
                if is(start, hash) || is(resume, hash) {
                    *starts += 1;
                }
                let generation = *starts;
                if is(start, hash) {
                    let due = ctx.time + *duration;
                    *state = TimerState::Running { due, generation };
                    return Reaction {
                        text: format!("starts: expires in {duration} s"),
                        actions: vec![Action::Timer { due, generation }],
                        unknown: false,
                    };
                }
                if is(stop, hash) {
                    *state = TimerState::Idle;
                    let mut r = Reaction::said("stops");
                    if let Some(m) = on_stopped {
                        r.text = format!("stops: sends {}", m.name);
                        r.actions.push(Action::Send {
                            msg: m.clone(),
                            delay: 0.0,
                            target: None,
                        });
                    }
                    return r;
                }
                if is(pause, hash) {
                    if let TimerState::Running { due, .. } = *state {
                        *state = TimerState::Paused {
                            left: (due - ctx.time).max(0.0),
                        };
                        return Reaction::said("pauses");
                    }
                    return Reaction::said("pause: not running");
                }
                if is(resume, hash) {
                    if let TimerState::Paused { left } = *state {
                        let due = ctx.time + left;
                        *state = TimerState::Running { due, generation };
                        return Reaction {
                            text: format!("resumes: expires in {left} s"),
                            actions: vec![Action::Timer { due, generation }],
                            unknown: false,
                        };
                    }
                    return Reaction::said("resume: not paused");
                }
                Reaction::said("heard; no reaction in the handler")
            }
            Self::TestCheckpoint {
                test,
                comparison,
                checkpoint,
                on_true,
                on_false,
            } => {
                if !is(test, hash) {
                    return Reaction::said("heard; no reaction in the handler");
                }
                let Some(current) = ctx.checkpoint else {
                    return Reaction {
                        text: "the current checkpoint is not known (input `checkpoint:N`)"
                            .to_owned(),
                        actions: Vec::new(),
                        unknown: true,
                    };
                };
                let (c, v) = (current as u32, *checkpoint as u32);
                let result = match *comparison {
                    0 => v < c,
                    1 => v <= c,
                    2 => c == v,
                    3 => c <= v,
                    4 => c < v,
                    _ => false,
                };
                let out = if result { on_true } else { on_false };
                let mut r = Reaction::said(format!("checkpoint {current}: {result}"));
                if let Some(m) = out {
                    r.text.push_str(&format!(", sends {}", m.name));
                    r.actions.push(Action::Send {
                        msg: m.clone(),
                        delay: 0.0,
                        target: None,
                    });
                }
                r
            }
            Self::Trigger {
                toggle,
                enable,
                disable,
                switch,
                ..
            } => {
                if is(toggle, hash) {
                    *switch = match *switch {
                        Switch::Off => Switch::On,
                        Switch::On => Switch::Off,
                        // The handler toggles a bit whose start value is not
                        // known.
                        Switch::Unknown => Switch::Unknown,
                    };
                    return Reaction::said(format!("toggled: {}", switch_name(*switch)));
                }
                if is(enable, hash) {
                    *switch = Switch::On;
                    return Reaction::said("switched on");
                }
                if is(disable, hash) {
                    *switch = Switch::Off;
                    return Reaction::said("switched off");
                }
                Reaction::said("heard; no reaction in the handler")
            }
            Self::LuaScript { start, stop, .. } => {
                if is(start, hash) {
                    return Reaction {
                        text: "starts the script".to_owned(),
                        actions: vec![Action::LuaStart],
                        unknown: false,
                    };
                }
                if is(stop, hash) {
                    return Reaction {
                        text: "stops the script".to_owned(),
                        actions: vec![Action::LuaStop],
                        unknown: false,
                    };
                }
                Reaction::said("heard; no reaction in the handler")
            }
            Self::BlackBox => Reaction {
                text: "reaction not known".to_owned(),
                actions: Vec::new(),
                unknown: true,
            },
        }
    }

    /// A `VariableWatcher` checks its variable (on activation and on every
    /// change while active).
    pub(super) fn check(&mut self, ctx: &mut Context<'_>) -> Reaction {
        let Self::VariableWatcher {
            send,
            variable,
            comparison,
            value,
            keep,
            max_fires,
            fired,
            active,
            ..
        } = self
        else {
            return Reaction::default();
        };
        if !*active {
            return Reaction::default();
        }
        let Some(v) = *variable else {
            *active = false;
            return Reaction::said("no variable");
        };
        if *max_fires != 0 && *fired >= *max_fires {
            *active = false;
            return Reaction::said("used up (max_fires reached)");
        }
        let (x, _) = (ctx.variable)(v);
        let hit = (*comparison & 2 != 0 && x == *value)
            || (*comparison & 1 != 0 && x < *value)
            || (*comparison & 4 != 0 && x > *value);
        if !hit {
            return Reaction::default();
        }
        *fired += 1;
        if !*keep {
            *active = false;
        }
        let mut r = Reaction::said(format!("variable {x}: condition met"));
        if let Some(out) = send {
            r.text.push_str(&format!(", sends {}", sends_text(&[out])));
            r.send(out);
        }
        r
    }

    /// The variable a watcher watches, while it is active.
    pub(super) fn watching(&self) -> Option<u32> {
        match self {
            Self::VariableWatcher {
                variable: Some(v),
                active: true,
                ..
            } => Some(*v),
            _ => None,
        }
    }
}

pub(super) fn switch_name(s: Switch) -> &'static str {
    match s {
        Switch::Unknown => "unknown",
        Switch::On => "on",
        Switch::Off => "off",
    }
}

//! A mock of the game's Lua API to run scripts outside the game.
//!
//! [`Sim`] loads a chunk into the [`Vm`] with the functions the game
//! registers (messages, receivers, timers, variables, positions, groups,
//! audio, `RegisterFunction`/`CallLuaFunction`) implemented as a small model:
//! messages are delivered at once to the receivers listening to them and to
//! relays (like `MessageRelay` objects), timers fire as game time advances,
//! variables are a table. Everything that reaches the "game" is logged
//! ([`SimEvent`]). The model follows what was observed in the game
//! (`docs/scripts.md`); where the game was not observed, the mock is the
//! simplest behaviour the names suggest.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use serde::Serialize;

use crate::resource::lua::Chunk;
use crate::resource::lua::vm::{LuaError, TableRef, Value, Vm, native};
use crate::{Error, Result};

/// Something a script did to the game.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum SimEvent {
    /// `SendMessage` (and `SendMessageEx`).
    Message {
        time: f64,
        message: String,
        data: Option<String>,
    },
    /// `loadfile(name)`: a file probe.
    Probe { time: f64, file: String },
    /// A game variable was set.
    Variable {
        time: f64,
        name: String,
        value: String,
    },
    /// Any other game function (audio, groups…).
    Call {
        time: f64,
        function: String,
        args: Vec<String>,
    },
    /// A timer fired.
    Timer { time: f64, callback: String },
    /// An error escaped to the game (it would be swallowed there).
    Error { time: f64, message: String },
}

struct Receiver {
    object: Value,
    function: Value,
    messages: Vec<String>,
    alive: bool,
}

struct Timer {
    callback: String,
    once: bool,
    /// Counted by the real clock in the game (`CreateTimer`'s third
    /// argument); the mock has one clock.
    #[allow(dead_code)]
    real_time: bool,
    period: f64,
    due: Option<f64>,
    deleted: bool,
}

/// State of the mock game.
#[derive(Default)]
pub struct SimState {
    /// Game time in seconds.
    pub time: f64,
    pub log: Vec<SimEvent>,
    /// Game variables by lower-case name.
    pub variables: BTreeMap<String, Value>,
    /// Player position (Y up).
    pub player: [f64; 3],
    /// Message relays: when the first arrives, the second is sent.
    pub relays: Vec<(String, String)>,
    /// `SendMessage` only logs the message: the host delivers it (the run of
    /// a mission's logic, `logic_graph::run`, has its own message bus).
    pub external_bus: bool,
    /// What `SendMessageEx` returns for a message (lower-case name): the
    /// values a listener of the game appends, e.g. the venue of
    /// `iMsgLuaGetPlayerVenue`.
    pub answers: BTreeMap<String, Vec<Value>>,
    /// Objects `Base` finds: GUIDs as [`normal_guid`] writes them.
    pub objects: BTreeSet<String>,
    receivers: Vec<Receiver>,
    timers: Vec<Timer>,
    groups: BTreeMap<String, Value>,
    registry: BTreeMap<String, BTreeMap<String, Value>>,
}

/// A script running against the mock game.
pub struct Sim {
    pub vm: Vm,
    pub state: Rc<RefCell<SimState>>,
}

impl Default for Sim {
    fn default() -> Self {
        Self::new()
    }
}

fn arg(args: &[Value], i: usize) -> Value {
    args.get(i).cloned().unwrap_or_default()
}

fn text(vm: &Vm, v: &Value) -> String {
    vm.tostring(v)
}

/// Functions of the game's API that the mock only logs (and answers with nil).
const LOGGED: &[&str] = &[
    "PlayEntityEmx",
    "StopEmx",
    "SetEmxVolume",
    "SetEmxPitch",
    "SetEmxPosition",
    "SetEmxAzimuth",
    "PlayStream",
    "PrimeStream",
    "StartStream",
    "StopStream",
    "SetStreamMixCategory",
    "SetStreamEmitterLocation",
    "SetStreamVolume",
    "SetStreamChannelVolume",
    "GetStreamStatus",
    "GetStreamDuration",
    "SetStreamPrimeFunction",
    "SetStreamEffect",
    "SetGlobalEffect",
    "SetPluginEffect",
    "SetAuxEnabled",
    "SetStreamChannelEffectSend",
    "PrimeEarsAudioStream",
    "StopEarsAudioStream",
    "SetEarsAudioStreamStopMessage",
    "SetEnvelopeMap",
    "GetEnvelopeValue",
    "SetMixValues",
    "ApplyMix",
    "RemoveMix",
    "SetMasterVolume",
    "GetDuckCategoryName",
    "GetImpactData",
    "GetVehicleType",
    "GetVehicleSpeed",
    "GetVehicleSpeedDelta",
    "GetVehicleAcceleration",
    "GetVehicleSkidEnergy",
    "GetVehicleHealth",
    "GetVehicleRainVelocity",
    "SetPlayerCarUsingSimpleAudio",
    "GetChaseVehicleStats",
    "SetChaseVehicleSound",
    "GetFootPoliceIntensity",
    "GetCarPoliceIntensity",
    "EclipseLog",
    "MessageReceiverNew",
    "MessageReceiverReceive",
];

impl Sim {
    /// A mock game with an empty state.
    pub fn new() -> Self {
        let state = Rc::new(RefCell::new(SimState::default()));
        let mut vm = Vm::new();
        install(&mut vm, &state);
        Self { vm, state }
    }

    /// Runs a chunk (registers its functions; nothing starts yet).
    pub fn load(&mut self, chunk: &[u8]) -> Result<()> {
        let chunk = Chunk::parse(chunk)?;
        self.vm.run_chunk(&chunk).map_err(lua_error)?;
        Ok(())
    }

    /// Whether a chunk registered `function` for `script` (`RegisterFunction`).
    pub fn registered(&self, script: &str, function: &str) -> bool {
        self.state
            .borrow()
            .registry
            .get(script)
            .is_some_and(|t| t.contains_key(function))
    }

    /// Calls `CallLuaFunction(script, "startup")` as `LuaAudioScript` does.
    pub fn start(&mut self, script: &str) -> Result<()> {
        self.call_registered(script, "startup")
    }

    /// Calls the script's `shutdown`.
    pub fn stop(&mut self, script: &str) -> Result<()> {
        self.call_registered(script, "shutdown")
    }

    fn call_registered(&mut self, script: &str, function: &str) -> Result<()> {
        let f = self
            .state
            .borrow()
            .registry
            .get(script)
            .and_then(|t| t.get(function))
            .cloned();
        match f {
            Some(f) => self.vm.call(&f, Vec::new()).map(|_| ()).map_err(lua_error),
            None => Err(Error::InvalidArgument(format!(
                "{script} did not register {function:?}"
            ))),
        }
    }

    /// The game sends a message.
    pub fn send(&mut self, message: &str) -> Result<()> {
        deliver(&mut self.vm, &self.state, message, Value::Nil, Value::Nil).map_err(lua_error)
    }

    /// The game sends a message with data.
    pub fn send_data(&mut self, message: &str, dtype: &str, data: Value) -> Result<()> {
        deliver(&mut self.vm, &self.state, message, Value::str(dtype), data).map_err(lua_error)
    }

    /// Advances game time, firing timers in order.
    pub fn advance(&mut self, seconds: f64) -> Result<()> {
        let end = self.state.borrow().time + seconds.max(0.0);
        for _ in 0..1_000_000 {
            let next = {
                let s = self.state.borrow();
                s.timers
                    .iter()
                    .enumerate()
                    .filter(|(_, t)| !t.deleted)
                    .filter_map(|(i, t)| t.due.map(|d| (d, i)))
                    .filter(|(d, _)| *d <= end)
                    .min_by(|a, b| a.0.total_cmp(&b.0))
            };
            let Some((due, i)) = next else {
                break;
            };
            let callback = {
                let mut s = self.state.borrow_mut();
                s.time = due;
                let t = &mut s.timers[i];
                if t.once {
                    t.due = None;
                } else {
                    t.due = Some(due + t.period.max(0.001));
                }
                let cb = t.callback.clone();
                s.log.push(SimEvent::Timer {
                    time: due,
                    callback: cb.clone(),
                });
                cb
            };
            // The game looks the name up each time, calls it with the number
            // of the timer and skips a name that is not a function (0x467440).
            let f = self.vm.get_global(&callback);
            if !matches!(f, Value::Function(_)) {
                continue;
            }
            if let Err(e) = self.vm.call(&f, vec![Value::Number(i as f64)]) {
                let time = self.state.borrow().time;
                self.state.borrow_mut().log.push(SimEvent::Error {
                    time,
                    message: e.text(),
                });
                if e.fatal {
                    return Err(lua_error(e));
                }
            }
        }
        self.state.borrow_mut().time = end;
        Ok(())
    }

    /// Value of a game variable.
    /// Makes `Base` find an object with this GUID (any form the game reads).
    pub fn add_object(&self, guid: &str) -> bool {
        match normal_guid(guid) {
            Some(g) => self.state.borrow_mut().objects.insert(g),
            None => false,
        }
    }

    /// `tostring` of a global of the script's Lua state.
    pub fn vm_global_text(&mut self, name: &str) -> String {
        let v = self.vm.get_global(name);
        match self.vm.call_global("tostring", vec![v]) {
            Ok(r) => r
                .first()
                .map_or_else(|| "nil".to_owned(), |v| self.vm.tostring(v)),
            Err(e) => e.text(),
        }
    }

    /// Whether the Lua state has a global of this name.
    pub fn has_global(&self, name: &str) -> bool {
        !self.vm.get_global(name).is_nil()
    }

    /// Whether the global table `table` has the field.
    pub fn has_table_field(&self, table: &str, field: &str) -> bool {
        self.vm
            .get_global(table)
            .as_table()
            .is_some_and(|t| !t.get_str(field).is_nil())
    }

    /// What `SendMessageEx(message, …)` returns from now on.
    pub fn answer(&self, message: &str, values: Vec<Value>) {
        self.state
            .borrow_mut()
            .answers
            .insert(message.to_lowercase(), values);
    }

    pub fn variable(&self, name: &str) -> Option<Value> {
        self.state
            .borrow()
            .variables
            .get(&name.to_lowercase())
            .cloned()
    }

    /// Sets a game variable as another object of the game would.
    pub fn set_variable(&self, name: &str, value: Value) {
        self.state
            .borrow_mut()
            .variables
            .insert(name.to_lowercase(), value);
    }

    /// Names of the messages the script sent, in order.
    pub fn sent(&self) -> Vec<String> {
        self.state
            .borrow()
            .log
            .iter()
            .filter_map(|e| match e {
                SimEvent::Message { message, .. } => Some(message.clone()),
                _ => None,
            })
            .collect()
    }

    /// File names the script probed, in order.
    pub fn probes(&self) -> Vec<String> {
        self.state
            .borrow()
            .log
            .iter()
            .filter_map(|e| match e {
                SimEvent::Probe { file, .. } => Some(file.clone()),
                _ => None,
            })
            .collect()
    }

    /// Takes the log.
    pub fn take_log(&self) -> Vec<SimEvent> {
        std::mem::take(&mut self.state.borrow_mut().log)
    }

    /// Messages some receiver listens to.
    pub fn listened(&self) -> Vec<String> {
        let s = self.state.borrow();
        let mut v: Vec<String> = s
            .receivers
            .iter()
            .filter(|r| r.alive)
            .flat_map(|r| r.messages.iter().cloned())
            .collect();
        v.sort();
        v.dedup();
        v
    }

    /// Whether some receiver listens to `message` (letter case does not
    /// matter, as with the game's hash of names).
    pub fn listens(&self, message: &str) -> bool {
        self.state
            .borrow()
            .receivers
            .iter()
            .any(|r| r.alive && r.messages.iter().any(|m| m.eq_ignore_ascii_case(message)))
    }

    /// Scripts that registered functions (`RegisterFunction`), in order of
    /// name.
    pub fn scripts(&self) -> Vec<String> {
        self.state.borrow().registry.keys().cloned().collect()
    }

    /// Game time (seconds) when the next timer fires.
    pub fn next_timer(&self) -> Option<f64> {
        self.state
            .borrow()
            .timers
            .iter()
            .filter(|t| !t.deleted)
            .filter_map(|t| t.due)
            .min_by(f64::total_cmp)
    }

    /// Number of running timers.
    pub fn running_timers(&self) -> usize {
        self.state
            .borrow()
            .timers
            .iter()
            .filter(|t| !t.deleted && t.due.is_some())
            .count()
    }
}

fn lua_error(e: LuaError) -> Error {
    Error::InvalidArgument(format!("Lua error: {e}"))
}

fn deliver(
    vm: &mut Vm,
    state: &Rc<RefCell<SimState>>,
    message: &str,
    dtype: Value,
    data: Value,
) -> std::result::Result<(), LuaError> {
    let targets: Vec<(Value, Value)> = state
        .borrow()
        .receivers
        .iter()
        .filter(|r| r.alive && r.messages.iter().any(|m| m.eq_ignore_ascii_case(message)))
        .map(|r| (r.function.clone(), r.object.clone()))
        .collect();
    for (f, obj) in targets {
        vm.call(
            &f,
            vec![obj, Value::str(message), dtype.clone(), data.clone()],
        )?;
    }
    let relayed: Vec<String> = state
        .borrow()
        .relays
        .iter()
        .filter(|(from, _)| from.eq_ignore_ascii_case(message))
        .map(|(_, to)| to.clone())
        .collect();
    for to in relayed {
        deliver(vm, state, &to, Value::Nil, Value::Nil)?;
    }
    Ok(())
}

fn position(x: f64, y: f64, z: f64) -> Value {
    let t = TableRef::new();
    t.set_str("x", Value::Number(x));
    t.set_str("y", Value::Number(y));
    t.set_str("z", Value::Number(z));
    t.set_str("__position", Value::Bool(true));
    Value::Table(t)
}

#[allow(clippy::too_many_lines)]
fn install(vm: &mut Vm, state: &Rc<RefCell<SimState>>) {
    let st = state.clone();
    vm.register("loadfile", move |vm, args| {
        let file = text(vm, &arg(&args, 0));
        let time = st.borrow().time;
        st.borrow_mut().log.push(SimEvent::Probe { time, file });
        Ok(vec![Value::Nil, Value::str("cannot open file")])
    });

    // RegisterFunction / CallLuaFunction, as the exe's own Lua code does.
    let st = state.clone();
    vm.register("RegisterFunction", move |_, args| {
        let script = arg(&args, 0).as_str().unwrap_or_default();
        let name = arg(&args, 1).as_str().unwrap_or_default();
        st.borrow_mut()
            .registry
            .entry(script)
            .or_default()
            .insert(name, arg(&args, 2));
        Ok(Vec::new())
    });
    let st = state.clone();
    vm.register("CallLuaFunction", move |vm, mut args| {
        let script = arg(&args, 0).as_str().unwrap_or_default();
        let name = arg(&args, 1).as_str().unwrap_or_default();
        let f = st
            .borrow()
            .registry
            .get(&script)
            .and_then(|t| t.get(&name))
            .cloned();
        if let Some(f) = f {
            let rest = if args.len() > 2 {
                args.split_off(2)
            } else {
                Vec::new()
            };
            vm.call(&f, rest)?;
        }
        Ok(Vec::new())
    });
    vm.register("RegisterScriptName", |_, _| Ok(Vec::new()));
    // Empty functions in the Lua code of the exe (`mission-logic.md` §2.3).
    vm.register("debug_print", |_, _| Ok(Vec::new()));
    vm.register("debug_screenprint", |_, _| Ok(Vec::new()));

    // Messages.
    let st = state.clone();
    vm.register("SendMessage", move |vm, args| {
        let message = text(vm, &arg(&args, 0));
        let data = args.get(2).map(|d| text(vm, d));
        let time = st.borrow().time;
        st.borrow_mut().log.push(SimEvent::Message {
            time,
            message: message.clone(),
            data,
        });
        if !st.borrow().external_bus {
            deliver(vm, &st, &message, arg(&args, 1), arg(&args, 2))?;
        }
        // The game returns 1 without pushing: the script gets its last argument.
        Ok(args.last().cloned().into_iter().collect())
    });
    let st = state.clone();
    vm.register("SendMessageEx", move |vm, args| {
        let message = text(vm, &arg(&args, 0));
        let time = st.borrow().time;
        st.borrow_mut().log.push(SimEvent::Message {
            time,
            message: message.clone(),
            data: args.get(1).map(|d| text(vm, d)),
        });
        // What the listener of the game appends to the data comes back.
        let answer = st.borrow().answers.get(&message.to_lowercase()).cloned();
        Ok(answer.unwrap_or_default())
    });
    let st = state.clone();
    vm.register("NewMessageReceiver", move |_, args| {
        let handle = st.borrow().receivers.len();
        let obj = TableRef::new();
        let s1 = st.clone();
        obj.set_str(
            "AddMessage",
            native(move |vm, a| {
                let m = text(vm, &arg(&a, 1));
                if let Some(r) = s1.borrow_mut().receivers.get_mut(handle)
                    && !r.messages.contains(&m)
                {
                    r.messages.push(m);
                }
                Ok(Vec::new())
            }),
        );
        let s2 = st.clone();
        obj.set_str(
            "RemoveMessage",
            native(move |vm, a| {
                let m = text(vm, &arg(&a, 1));
                if let Some(r) = s2.borrow_mut().receivers.get_mut(handle) {
                    r.messages.retain(|x| *x != m);
                }
                Ok(Vec::new())
            }),
        );
        let s3 = st.clone();
        obj.set_str(
            "RemoveAllMessages",
            native(move |_, _| {
                if let Some(r) = s3.borrow_mut().receivers.get_mut(handle) {
                    r.messages.clear();
                }
                Ok(Vec::new())
            }),
        );
        let s4 = st.clone();
        obj.set_str(
            "Delete",
            native(move |_, _| {
                if let Some(r) = s4.borrow_mut().receivers.get_mut(handle) {
                    r.messages.clear();
                    r.alive = false;
                }
                Ok(Vec::new())
            }),
        );
        let object = Value::Table(obj);
        st.borrow_mut().receivers.push(Receiver {
            object: object.clone(),
            function: arg(&args, 0),
            messages: Vec::new(),
            alive: true,
        });
        Ok(vec![object])
    });

    // Timers: a timer is its index.
    let st = state.clone();
    vm.register("CreateTimer", move |vm, args| {
        let callback = text(vm, &arg(&args, 0));
        // As in the game (0x467710): a name of 1..63 characters and a boolean
        // "one shot", or the handle is -1.
        if !matches!(arg(&args, 0), Value::Str(_))
            || callback.is_empty()
            || callback.len() > 63
            || !matches!(arg(&args, 1), Value::Bool(_))
        {
            return Ok(vec![Value::Number(-1.0)]);
        }
        let mut s = st.borrow_mut();
        s.timers.push(Timer {
            callback,
            once: arg(&args, 1).truthy(),
            real_time: arg(&args, 2).truthy(),
            period: 1.0,
            due: None,
            deleted: false,
        });
        Ok(vec![Value::Number((s.timers.len() - 1) as f64)])
    });
    let timer_index = |args: &[Value]| arg(args, 0).as_number().map(|n| n as usize);
    let st = state.clone();
    vm.register("StartTimer", move |_, args| {
        let mut s = st.borrow_mut();
        let now = s.time;
        if let Some(t) = timer_index(&args).and_then(|i| s.timers.get_mut(i))
            && !t.deleted
        {
            if let Some(ms) = arg(&args, 1).as_number() {
                t.period = ms / 1000.0;
            }
            t.due = Some(now + t.period);
            return Ok(vec![Value::Number((t.period * 1000.0).round())]);
        }
        Ok(vec![Value::Number(0.0)])
    });
    let st = state.clone();
    vm.register("StopTimer", move |_, args| {
        if let Some(i) = timer_index(&args)
            && let Some(t) = st.borrow_mut().timers.get_mut(i)
        {
            t.due = None;
        }
        Ok(Vec::new())
    });
    let st = state.clone();
    vm.register("DeleteTimer", move |_, args| {
        if let Some(i) = timer_index(&args)
            && let Some(t) = st.borrow_mut().timers.get_mut(i)
        {
            t.deleted = true;
            t.due = None;
        }
        Ok(Vec::new())
    });
    let st = state.clone();
    vm.register("SetTimerRate", move |_, args| {
        if let Some(i) = timer_index(&args)
            && let Some(t) = st.borrow_mut().timers.get_mut(i)
            && let Some(ms) = arg(&args, 1).as_number()
        {
            t.period = ms / 1000.0;
        }
        Ok(Vec::new())
    });
    let st = state.clone();
    vm.register("QueryTimer", move |_, args| {
        // Milliseconds left (0x467D50); 0 for a timer that does not exist.
        let s = st.borrow();
        let left = timer_index(&args)
            .and_then(|i| s.timers.get(i))
            .filter(|t| !t.deleted)
            .map_or(0.0, |t| {
                t.due.map_or(0.0, |due| ((due - s.time) * 1000.0).round())
            });
        Ok(vec![Value::Number(left)])
    });

    // Time and positions.
    let st = state.clone();
    // Milliseconds, as in the game.
    vm.register("GetSimTime", move |vm, _| {
        let ms = (st.borrow().time * 1000.0).round();
        Ok(vec![Value::Number(if vm.float {
            ms as f32 as f64
        } else {
            ms
        })])
    });
    let st = state.clone();
    vm.register("GetPlayerPosition", move |_, _| {
        let [x, y, z] = st.borrow().player;
        Ok(vec![position(x, y, z)])
    });
    vm.register("GetPositionValues", |_, args| {
        // The game reads three floats at a pointer: the data of a message
        // sent with a number is a pointer to that number (the other two are
        // whatever follows); anything that is no pointer gives 0, 0, 0.
        let zero = Value::Number(0.0);
        Ok(match arg(&args, 0) {
            Value::Table(t) => vec![t.get_str("x"), t.get_str("y"), t.get_str("z")],
            Value::Number(n) => vec![Value::Number(n), zero.clone(), zero],
            _ => vec![zero.clone(), zero.clone(), zero],
        })
    });
    vm.register("SetPositionValues", |_, args| {
        let Some(t) = arg(&args, 0).as_table().cloned() else {
            return Err(LuaError::msg("SetPositionValues expects a position"));
        };
        t.set_str("x", arg(&args, 1));
        t.set_str("y", arg(&args, 2));
        t.set_str("z", arg(&args, 3));
        Ok(Vec::new())
    });
    vm.register("GetPlatform", |_, _| Ok(vec![Value::str("windows")]));
    // 32-bit integers are userdata in the game (GetSimTime, VariableGet and
    // GetMemberCount return them); the mock keeps plain numbers, so the three
    // conversions only cut the fraction.
    for name in ["Int32", "UInt32", "Int32ToNumber"] {
        vm.register(name, move |vm, args| {
            let v = arg(&args, 0);
            match v.as_number() {
                Some(n) => Ok(vec![Value::Number(n.trunc())]),
                None => Err(LuaError::msg(format!(
                    "Could not turn '{}' into an Int32.",
                    text(vm, &v)
                ))),
            }
        });
    }
    for name in ["TEMP_POS", "CreatePosition"] {
        vm.register(name, |_, args| {
            let n = |i| arg(&args, i).as_number().unwrap_or(0.0);
            Ok(vec![position(n(0), n(1), n(2))])
        });
    }

    // Objects: `Base("{guid}")` is a weak reference; empty when the game has
    // no such object. The game reads only four bytes of the text
    // ([`guid_the_game_reads`]), so it finds no object of the game itself.
    let st = state.clone();
    vm.register("Base", move |vm, args| {
        let v = arg(&args, 0);
        if is_base(&v) {
            return Ok(vec![v]);
        }
        let Value::Str(_) = v else {
            return Err(LuaError::msg(
                "ToBasePtr: 1st argument should be a string or Base.",
            ));
        };
        let name = text(vm, &v);
        let Some(guid) = normal_guid(&name) else {
            return Err(LuaError::msg(format!(
                "ToBasePtr: Could not turn '{name}' into a GUID."
            )));
        };
        let guid = guid_the_game_reads(&guid);
        let known = st.borrow().objects.contains(&guid);
        Ok(vec![base(known.then_some(guid.as_str()))])
    });
    vm.register("DeleteBase", |_, args| {
        let v = arg(&args, 0);
        if is_base(&v)
            && let Some(t) = v.as_table()
        {
            t.set_str("__base", Value::Bool(false));
        }
        Ok(args.last().cloned().into_iter().collect())
    });

    // Variables: a handle is a table with Get and Set.
    let st = state.clone();
    vm.register("FindVariable", move |vm, args| {
        let name = text(vm, &arg(&args, 0)).to_lowercase();
        st.borrow_mut()
            .variables
            .entry(name.clone())
            .or_insert(Value::Number(0.0));
        let v = TableRef::new();
        v.set_str("__variable", Value::str(&name));
        let s1 = st.clone();
        let n1 = name.clone();
        v.set_str(
            "Get",
            native(move |_, _| {
                Ok(vec![
                    s1.borrow().variables.get(&n1).cloned().unwrap_or_default(),
                ])
            }),
        );
        let s2 = st.clone();
        v.set_str(
            "Set",
            native(move |vm, a| {
                set_var(vm, &s2, &name, arg(&a, 1))?;
                Ok(Vec::new())
            }),
        );
        Ok(vec![Value::Table(v)])
    });
    let st = state.clone();
    vm.register("VariableGet", move |_, args| {
        let name = arg(&args, 0)
            .as_table()
            .and_then(|t| t.get_str("__variable").as_str())
            .ok_or_else(|| {
                LuaError::msg("LuaVariableGet: Expected variable handle, but found something else.")
            })?;
        Ok(vec![
            st.borrow()
                .variables
                .get(&name)
                .cloned()
                .unwrap_or_default(),
        ])
    });
    let st = state.clone();
    vm.register("VariableSet", move |vm, args| {
        let name = arg(&args, 0)
            .as_table()
            .and_then(|t| t.get_str("__variable").as_str())
            .ok_or_else(|| {
                LuaError::msg("VariableSet: Expected variable handle as 1st argument.")
            })?;
        set_var(vm, &st, &name, arg(&args, 1))?;
        Ok(Vec::new())
    });

    // Groups.
    let gm = TableRef::new();
    let st = state.clone();
    gm.set_str(
        "RegisterGroup",
        native(move |vm, args| {
            let name = text(vm, &arg(&args, 1));
            let g = st
                .borrow_mut()
                .groups
                .entry(name)
                .or_insert_with(|| new_group(&st))
                .clone();
            Ok(vec![g])
        }),
    );
    let st = state.clone();
    gm.set_str(
        "FindGroup",
        native(move |vm, args| {
            let name = text(vm, &arg(&args, 1));
            Ok(vec![
                st.borrow().groups.get(&name).cloned().unwrap_or_default(),
            ])
        }),
    );
    let st = state.clone();
    gm.set_str(
        "RemoveFromAllGroups",
        native(move |vm, a| {
            member(
                vm,
                &st.borrow().objects,
                &arg(&a, 1),
                "GroupManager:RemoveFromAllGroups",
            )?;
            Ok(a.last().cloned().into_iter().collect())
        }),
    );
    vm.set_global("GroupManager", Value::Table(gm));

    // Audio: handles are numbers; everything else is logged.
    for name in ["PlayEmx", "PlayEarsAudioStream"] {
        let st = state.clone();
        vm.register(name, move |vm, args| {
            log_call(vm, &st, name, &args);
            let n = st.borrow().log.len();
            Ok(vec![Value::Number(n as f64)])
        });
    }
    for &name in LOGGED {
        let st = state.clone();
        vm.register(name, move |vm, args| {
            log_call(vm, &st, name, &args);
            // On foot, as the mock's player always is (game-bench.md §35).
            Ok(match name {
                "GetVehicleSpeed" => vec![Value::Number(0.0), Value::Bool(false)],
                "GetVehicleType"
                | "GetVehicleSpeedDelta"
                | "GetVehicleAcceleration"
                | "GetVehicleSkidEnergy"
                | "GetVehicleHealth"
                | "GetVehicleRainVelocity" => vec![Value::Number(0.0)],
                _ => vec![Value::Nil],
            })
        });
    }
}

/// Sets a game variable. Variables made by `FindVariable` hold 32-bit
/// integers in the game: numbers are cut to whole numbers, anything else is
/// refused with the game's message.
fn set_var(
    vm: &Vm,
    st: &Rc<RefCell<SimState>>,
    name: &str,
    value: Value,
) -> std::result::Result<(), LuaError> {
    let value = match value.as_number() {
        Some(n) => Value::Number(n.trunc()),
        None => {
            return Err(LuaError::msg(format!(
                "Could not turn ({}) into an Int32.",
                if value.is_nil() {
                    "null".to_owned()
                } else {
                    text(vm, &value)
                }
            )));
        }
    };
    let time = st.borrow().time;
    let shown = text(vm, &value);
    let mut s = st.borrow_mut();
    s.variables.insert(name.to_owned(), value);
    s.log.push(SimEvent::Variable {
        time,
        name: name.to_owned(),
        value: shown,
    });
    Ok(())
}

fn log_call(vm: &Vm, st: &Rc<RefCell<SimState>>, function: &str, args: &[Value]) {
    let time = st.borrow().time;
    let args = args.iter().map(|a| text(vm, a)).collect();
    st.borrow_mut().log.push(SimEvent::Call {
        time,
        function: function.to_owned(),
        args,
    });
}

/// A `Base` of the mock: a table whose `tostring` is the GUID, or nil for an
/// empty reference (as in the game).
fn base(guid: Option<&str>) -> Value {
    let t = TableRef::new();
    t.set_str("__base", guid.map_or(Value::Bool(false), Value::str));
    let meta = TableRef::new();
    meta.set_str(
        "__tostring",
        native(|_, a| {
            let guid = arg(&a, 0)
                .as_table()
                .map(|t| t.get_str("__base"))
                .unwrap_or_default();
            Ok(match guid {
                Value::Str(_) => vec![guid],
                _ => Vec::new(),
            })
        }),
    );
    t.set_metatable(Some(meta));
    Value::Table(t)
}

fn is_base(v: &Value) -> bool {
    v.as_table().is_some_and(|t| !t.get_str("__base").is_nil())
}

/// The member a group function is given: a `Base` or the text of a GUID.
/// An empty reference crashes the game (a read of address 0 at 0x00469F85).
fn member(
    vm: &Vm,
    objects: &BTreeSet<String>,
    v: &Value,
    function: &str,
) -> std::result::Result<String, LuaError> {
    match v {
        // The text of a GUID goes the way of `Base`: an object the game does
        // not find is the same crash.
        Value::Str(_) => {
            let guid = normal_guid(&text(vm, v)).ok_or_else(|| {
                LuaError::msg(format!(
                    "ToBasePtr: Could not turn '{}' into a GUID.",
                    text(vm, v)
                ))
            })?;
            let guid = guid_the_game_reads(&guid);
            if objects.contains(&guid) {
                Ok(guid)
            } else {
                Err(LuaError::msg(format!(
                    "{function} with a GUID the game does not find: the game crashes here (0x00469F85)"
                )))
            }
        }
        _ if is_base(v) => match v.as_table().map(|t| t.get_str("__base")) {
            Some(Value::Str(s)) => Ok(String::from_utf8_lossy(&s).into_owned()),
            _ => Err(LuaError::msg(format!(
                "{function} with an empty Base: the game crashes here (0x00469F85)"
            ))),
        },
        _ => Err(LuaError::msg(
            "ToBasePtr: 1st argument should be a string or Base.",
        )),
    }
}

/// A GUID as the game prints it: `{XXXXXXXX-XXXX-XXXX-XXXX-XXXXXXXXXXXX}`.
/// The game's parser (0x46B4D0) takes it with or without braces, in either case.
/// The GUID `Base` really looks for. The parser of the PC build (0x46B4D0)
/// shifts a byte where it should shift a word, so of the 32 digits only eight
/// survive: bytes 0, 4, 8 and 12 of the GUID, each alone in its 32-bit word.
/// `{32C8D726-FA3D-AA11-464F-574C45523130}` is looked up as
/// `{00000026-003D-0000-4600-000045000000}`. Takes and gives [`normal_guid`] text.
pub fn guid_the_game_reads(normal: &str) -> String {
    let t = normal.as_bytes();
    let pair = |at: usize| String::from_utf8_lossy(&t[at..at + 2]).into_owned();
    // Offsets in "{XXXXXXXX-XXXX-XXXX-XXXX-XXXXXXXXXXXX}".
    format!(
        "{{000000{}-00{}-0000-{}00-0000{}000000}}",
        pair(7),
        pair(12),
        pair(20),
        pair(29)
    )
}

pub fn normal_guid(text: &str) -> Option<String> {
    let t = text.trim_start();
    let t = t.strip_prefix('{').unwrap_or(t);
    let body = t.get(..36)?;
    let ok = body.bytes().enumerate().all(|(i, c)| match i {
        8 | 13 | 18 | 23 => c == b'-',
        _ => c.is_ascii_hexdigit(),
    });
    ok.then(|| format!("{{{}}}", body.to_ascii_uppercase()))
}

fn new_group(st: &Rc<RefCell<SimState>>) -> Value {
    let members = Rc::new(RefCell::new(Vec::<String>::new()));
    let g = TableRef::new();
    let m = members.clone();
    let state = st.clone();
    g.set_str(
        "AddMember",
        native(move |vm, a| {
            let guid = member(vm, &state.borrow().objects, &arg(&a, 1), "Group:AddMember")?;
            if !m.borrow().contains(&guid) {
                m.borrow_mut().push(guid);
            }
            Ok(a.last().cloned().into_iter().collect())
        }),
    );
    let m = members.clone();
    let state = st.clone();
    g.set_str(
        "RemoveMember",
        native(move |vm, a| {
            let guid = member(
                vm,
                &state.borrow().objects,
                &arg(&a, 1),
                "Group:RemoveMember",
            )?;
            m.borrow_mut().retain(|x| *x != guid);
            Ok(a.last().cloned().into_iter().collect())
        }),
    );
    // Stubs in the game: IsMember checks nothing (0x46A080) and Clear clears
    // nothing (0x46A0F0); both give the script its last argument back.
    let state = st.clone();
    g.set_str(
        "IsMember",
        native(move |vm, a| {
            member(vm, &state.borrow().objects, &arg(&a, 1), "Group:IsMember")?;
            Ok(a.last().cloned().into_iter().collect())
        }),
    );
    g.set_str(
        "Clear",
        native(move |_, a| Ok(a.last().cloned().into_iter().collect())),
    );
    let m = members.clone();
    g.set_str(
        "GetMemberCount",
        native(move |_, _| Ok(vec![Value::Number(m.borrow().len() as f64)])),
    );
    g.set_str(
        "ForEachMember",
        native(move |vm, a| {
            let f = arg(&a, 1);
            let list = members.borrow().clone();
            for guid in list {
                vm.call(&f, vec![base(Some(&guid))])?;
            }
            Ok(a.last().cloned().into_iter().collect())
        }),
    );
    Value::Table(g)
}

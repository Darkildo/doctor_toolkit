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
use std::collections::BTreeMap;
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
    auto_delete: bool,
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
                    t.deleted = t.auto_delete;
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
            let f = self.vm.get_global(&callback);
            if let Err(e) = self.vm.call(&f, Vec::new()) {
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
        .filter(|r| r.alive && r.messages.iter().any(|m| m == message))
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
        .filter(|(from, _)| from == message)
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
        deliver(vm, &st, &message, arg(&args, 1), arg(&args, 2))?;
        Ok(Vec::new())
    });
    let st = state.clone();
    vm.register("SendMessageEx", move |vm, args| {
        let message = text(vm, &arg(&args, 0));
        let time = st.borrow().time;
        st.borrow_mut().log.push(SimEvent::Message {
            time,
            message,
            data: args.get(1).map(|d| text(vm, d)),
        });
        Ok(vec![Value::Nil])
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
        let mut s = st.borrow_mut();
        s.timers.push(Timer {
            callback,
            once: arg(&args, 1).truthy(),
            auto_delete: arg(&args, 2).truthy(),
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
        }
        Ok(Vec::new())
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
        let running = timer_index(&args)
            .and_then(|i| st.borrow().timers.get(i).map(|t| t.due.is_some()))
            .unwrap_or(false);
        Ok(vec![Value::Bool(running)])
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
    vm.register("CreatePosition", |_, _| Ok(vec![position(0.0, 0.0, 0.0)]));
    vm.register("GetPositionValues", |_, args| {
        let Some(t) = arg(&args, 0).as_table().cloned() else {
            return Err(LuaError::msg("GetPositionValues expects a position"));
        };
        Ok(vec![t.get_str("x"), t.get_str("y"), t.get_str("z")])
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
    vm.register("GetPlatform", |_, _| Ok(vec![Value::str("PC")]));

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
                .or_insert_with(new_group)
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
            Ok(vec![Value::Nil])
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

fn new_group() -> Value {
    let members = Rc::new(RefCell::new(Vec::<Value>::new()));
    let g = TableRef::new();
    let m = members.clone();
    g.set_str(
        "AddMember",
        native(move |_, a| {
            let v = arg(&a, 1);
            if !m.borrow().contains(&v) {
                m.borrow_mut().push(v);
            }
            Ok(Vec::new())
        }),
    );
    let m = members.clone();
    g.set_str(
        "RemoveMember",
        native(move |_, a| {
            let v = arg(&a, 1);
            m.borrow_mut().retain(|x| *x != v);
            Ok(Vec::new())
        }),
    );
    let m = members.clone();
    g.set_str(
        "IsMember",
        native(move |_, a| Ok(vec![Value::Bool(m.borrow().contains(&arg(&a, 1)))])),
    );
    let m = members.clone();
    g.set_str(
        "Clear",
        native(move |_, _| {
            m.borrow_mut().clear();
            Ok(Vec::new())
        }),
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
            for v in list {
                vm.call(&f, vec![v])?;
            }
            Ok(Vec::new())
        }),
    );
    Value::Table(g)
}

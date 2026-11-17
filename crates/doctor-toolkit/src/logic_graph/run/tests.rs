use super::*;
use crate::resource::simgroup::SimGroup;

fn guid(n: u8) -> Guid {
    let mut g = [n; 16];
    g[8..].copy_from_slice(b"RUNTESTS");
    Guid(g)
}

/// A SimGroup built object by object, with values per packet class.
struct Builder {
    group: SimGroup,
    next: u8,
}

enum V {
    S(&'static str),
    F(f32),
    I(i32),
    H(&'static str),
    G(Guid),
}

impl Builder {
    fn new() -> Self {
        Self {
            group: SimGroup::empty(),
            next: 1,
        }
    }

    fn add(&mut self, class: &str, values: &[(&str, u16, V)]) -> Guid {
        let c = string_id(class);
        let layout = LogicSchema::gf2().layout(c).expect("layout").to_vec();
        let g = guid(self.next);
        self.next += 1;
        let o = self.group.push_object(c, g, &layout).unwrap();
        for (packet, index, v) in values {
            let value = match v {
                V::S(s) => Value::String((*s).into()),
                V::F(f) => Value::Float(*f),
                V::I(i) => Value::Int(*i),
                V::H(s) => Value::Hash(crate::resource::simgroup::Hash32(string_id(s))),
                V::G(g) => Value::Guid(*g),
            };
            self.group
                .set_class_attr(o, string_id(packet), *index, Some(value))
                .unwrap();
        }
        g
    }

    fn input(&self) -> RunInput {
        RunInput {
            groups: vec![LoadedGroup {
                location: Location {
                    stream: 0,
                    resource: 3,
                },
                stream: "test/stream".into(),
                group: SimGroup::parse(&self.group.encode()).unwrap(),
            }],
            chunks: HashMap::new(),
            variables: Vec::new(),
        }
    }

    fn run(&self) -> Run {
        Run::new(self.input(), RunOptions::default())
    }
}

/// Messages sent, in order, with their times.
fn sent(run: &Run) -> Vec<(f64, String)> {
    run.journal()
        .iter()
        .filter_map(|e| match &e.what {
            What::Send { message, .. } => Some((e.time, message.clone())),
            _ => None,
        })
        .collect()
}

fn names(run: &Run) -> Vec<String> {
    sent(run).into_iter().map(|(_, m)| m).collect()
}

fn mm(b: &mut Builder, trigger: &'static str, sends: &[(&'static str, f32)]) -> Guid {
    let mut values = vec![
        ("MultiManager", 0, V::S(trigger)),
        ("MultiManager", 1, V::I(-1)),
    ];
    for (k, (m, d)) in sends.iter().enumerate() {
        values.push(("MultiManager", 3 + 2 * k as u16, V::S(m)));
        values.push(("MultiManager", 4 + 2 * k as u16, V::F(*d)));
    }
    b.add("MultiManager", &values)
}

#[test]
fn forwarding_is_depth_first_and_delays_wait_for_game_time() {
    let mut b = Builder::new();
    b.add(
        "TriggerAuto",
        &[
            ("TriggerAuto", 0, V::S("go")),
            ("TriggerAuto", 1, V::F(0.5)),
        ],
    );
    mm(&mut b, "go", &[("a", 0.0), ("b", 0.0), ("late", 2.0)]);
    mm(&mut b, "a", &[("a_child", 0.0)]);
    b.add(
        "MessageRelay",
        &[
            ("MessageRelay", 0, V::S("b")),
            ("MessageRelay", 1, V::S("from_relay")),
        ],
    );
    let mut run = b.run();
    run.schedule(0.0, Input::Load);
    assert_eq!(run.run(Until::Idle, 1000), Stop::Idle);
    assert_eq!(
        sent(&run),
        vec![
            (0.0, POST_SPAWN.to_owned()),
            (0.5, "go".into()),
            (0.5, "a".into()),
            // The reaction to `a` comes before `b` is sent: synchronous bus.
            (0.5, "a_child".into()),
            (0.5, "b".into()),
            (0.5, "from_relay".into()),
            (2.5, "late".into()),
        ]
    );
    // TriggerAuto fires once.
    run.schedule(3.0, Input::Message(POST_SPAWN.into()));
    run.run(Until::Idle, 1000);
    assert_eq!(names(&run).iter().filter(|m| *m == "go").count(), 1);
}

#[test]
fn the_same_input_gives_the_same_journal() {
    let make = || {
        let mut b = Builder::new();
        b.add(
            "TriggerRandom",
            &[
                ("TriggerRandom", 0, V::S("roll")),
                ("TriggerRandom", 1, V::F(1.0)),
                ("TriggerRandom", 2, V::S("heads")),
                ("TriggerRandom", 4, V::F(1.0)),
                ("TriggerRandom", 5, V::S("tails")),
            ],
        );
        let mut run = b.run();
        for k in 0..20 {
            run.schedule(f64::from(k), Input::Message("roll".into()));
        }
        run.run(Until::Idle, 10_000);
        serde_json::to_string(run.journal()).unwrap()
    };
    let a = make();
    assert_eq!(a, make());
    assert!(
        a.contains("heads") && a.contains("tails"),
        "both sides come up"
    );
}

#[test]
fn variable_switch_and_operator_follow_the_variable() {
    let mut b = Builder::new();
    b.add(
        "VariableOperator",
        &[
            ("VariableOperator", 0, V::S("inc")),
            ("VariableOperator", 2, V::H("doors")),
            ("VariableOperator", 3, V::I(0)),
            ("VariableOperator", 4, V::I(1)),
        ],
    );
    b.add(
        "VariableSwitch",
        &[
            ("VariableSwitch", 0, V::S("check")),
            ("VariableSwitch", 1, V::H("doors")),
            ("VariableSwitch", 2, V::S("doors")),
            ("VariableSwitch", 3, V::S("none_yet")),
            ("VariableSwitch", 5, V::I(1)),
            ("VariableSwitch", 6, V::S("one_door")),
            ("VariableSwitch", 8, V::I(2)),
            ("VariableSwitch", 9, V::S("two_doors")),
            ("VariableSwitch", 10, V::F(1.0)),
        ],
    );
    b.add(
        "VariableWatcher",
        &[
            ("VariableWatcher", 0, V::S("watch")),
            ("VariableWatcher", 2, V::S("enough_doors")),
            ("VariableWatcher", 4, V::H("doors")),
            ("VariableWatcher", 5, V::I(4)),
            ("VariableWatcher", 6, V::I(1)),
        ],
    );
    let mut run = b.run();
    for (t, i) in parse_inputs(&[
        "send:check; send:watch; send:inc; send:check; wait:1; send:inc; send:check".into(),
    ])
    .unwrap()
    {
        run.schedule(t, i);
    }
    run.run(Until::Idle, 1000);
    let s = sent(&run);
    assert!(s.contains(&(0.0, "none_yet".into())));
    assert!(s.contains(&(0.0, "one_door".into())));
    // Value 2 sends `two_doors` after its delay of 1 s.
    assert!(s.contains(&(2.0, "two_doors".into())));
    // The watcher fires once, when the variable becomes greater than 1.
    let fired: Vec<_> = s.iter().filter(|(_, m)| m == "enough_doors").collect();
    assert_eq!(fired, vec![&(1.0, "enough_doors".to_owned())]);
    assert!(run.variables().contains(&("doors".to_owned(), 2, true)));
}

#[test]
fn mission_timer_expires_and_breakpoints_stop_the_run() {
    let mut b = Builder::new();
    b.add(
        "MissionTimer",
        &[
            ("MissionTimer", 0, V::S("timer_start")),
            ("MissionTimer", 1, V::S("timer_stop")),
            ("MissionTimer", 3, V::F(30.0)),
            ("MissionTimer", 7, V::S("time_up")),
        ],
    );
    let catcher = mm(&mut b, "time_up", &[("game_over", 0.0)]);
    let mut run = b.run();
    run.schedule(1.0, Input::Message("timer_start".into()));
    run.toggle_breakpoint(Breakpoint::Message(string_id("game_over")));
    let stop = run.run(Until::Idle, 1000);
    assert!(matches!(stop, Stop::Breakpoint(_)), "{stop:?}");
    assert_eq!(run.time(), 31.0);
    assert!(!names(&run).contains(&"game_over".to_owned()));
    // Going on passes the breakpoint.
    assert_eq!(run.run(Until::Idle, 1000), Stop::Idle);
    assert!(names(&run).contains(&"game_over".to_owned()));

    // A breakpoint on the object stops before it gets the message.
    let mut run = b.run();
    let actor = run.find(&catcher.to_string())[0];
    run.toggle_breakpoint(Breakpoint::Object(actor));
    run.schedule(0.0, Input::Message("timer_start".into()));
    run.schedule(10.0, Input::Message("timer_stop".into()));
    run.schedule(11.0, Input::Message("timer_start".into()));
    let stop = run.run(Until::Idle, 1000);
    assert!(matches!(stop, Stop::Breakpoint(_)));
    // The first start was stopped: the timer ends 30 s after the second.
    assert_eq!(run.time(), 41.0);

    // Until a time, and step by step to the next message.
    let mut run = b.run();
    run.schedule(0.0, Input::Message("timer_start".into()));
    assert_eq!(run.run(Until::Time(10.0), 1000), Stop::Time);
    assert_eq!(run.time(), 10.0);
    assert_eq!(run.run(Until::Message, 1000), Stop::Message);
    assert_eq!(
        names(&run),
        vec!["timer_start".to_owned(), "time_up".to_owned()]
    );
}

#[test]
fn a_loop_without_delay_is_cut_and_one_with_a_delay_runs_on() {
    let mut b = Builder::new();
    mm(&mut b, "ping", &[("pong", 0.0)]);
    mm(&mut b, "pong", &[("ping", 0.0)]);
    let mut run = b.run();
    run.schedule(0.0, Input::Message("ping".into()));
    assert_eq!(run.run(Until::Idle, 10_000), Stop::Idle);
    assert!(
        run.journal()
            .iter()
            .any(|e| matches!(&e.what, What::Warning { text } if text.contains("loop")))
    );

    let mut b = Builder::new();
    mm(&mut b, "tick", &[("tock", 1.0)]);
    mm(&mut b, "tock", &[("tick", 1.0)]);
    let mut run = b.run();
    run.schedule(0.0, Input::Message("tick".into()));
    assert_eq!(run.run(Until::Time(10.0), 10_000), Stop::Time);
    assert_eq!(names(&run).len(), 11);
    assert_eq!(run.run(Until::Steps(5), 10_000), Stop::Steps);
    assert_eq!(run.run(Until::Idle, 50), Stop::Limit);
}

#[test]
fn black_boxes_get_messages_and_take_manual_reactions() {
    let mut b = Builder::new();
    mm(&mut b, "start", &[("spawn_guards", 0.0)]);
    let spawner = b.add("FuncSpawn", &[("FuncSpawn", 0, V::S("spawn_guards"))]);
    mm(&mut b, "guards_dead", &[("mission_won", 0.0)]);
    let mut run = b.run();
    run.schedule(0.0, Input::Message("start".into()));
    run.run(Until::Idle, 1000);
    let delivered = run
        .journal()
        .iter()
        .find_map(|e| match &e.what {
            What::Deliver { to, took, .. } if to.class.as_deref() == Some("FuncSpawn") => {
                Some(*took)
            }
            _ => None,
        })
        .unwrap();
    assert_eq!(delivered, Took::Unknown);
    assert!(!names(&run).contains(&"mission_won".to_owned()));

    let mut run = b.run();
    let given = run.add_reaction(&ManualReaction {
        object: spawner.to_string(),
        on: None,
        send: vec![ManualOut {
            message: "guards_dead".into(),
            delay: 5.0,
        }],
    });
    assert_eq!(given.len(), 1);
    run.schedule(0.0, Input::Message("start".into()));
    run.run(Until::Idle, 1000);
    assert!(sent(&run).contains(&(5.0, "mission_won".into())));
}

#[test]
fn triggers_take_the_player_and_their_switch() {
    let mut b = Builder::new();
    // A touch sensor that is on at load (bit 1 of the flags, attribute 8).
    b.add(
        "TriggerBox",
        &[
            ("TriggerBase", 1, V::S("box_on")),
            ("TriggerBase", 2, V::S("box_off")),
            ("TriggerBase", 4, V::S("entered")),
            ("TriggerBase", 5, V::F(0.25)),
            ("TriggerBase", 6, V::I(-1)),
            ("TriggerBase", 7, V::S("reverse")),
            ("TriggerBase", 8, V::I(1)),
        ],
    );
    let mut run = b.run();
    let sel = "#0";
    assert_eq!(run.find(sel).len(), 1);
    assert_eq!(
        run.find(&format!("test/stream#3:{}", run.actors()[0].object))
            .len(),
        1
    );
    for (t, i) in parse_inputs(&[format!(
        "enter:{sel}; wait:1; exit:{sel}; send:box_off; enter:{sel}; at:5:send:box_on; at:6:enter:{sel}"
    )])
    .unwrap()
    {
        run.schedule(t, i);
    }
    run.run(Until::Idle, 1000);
    let s = sent(&run);
    assert_eq!(
        s.iter()
            .filter(|(_, m)| m == "entered")
            .map(|(t, _)| *t)
            .collect::<Vec<_>>(),
        vec![0.25, 6.25]
    );
    // Attribute 7 is the message of a touch against the axis of the volume
    // (0x004C1E40), not of leaving: nothing is sent when the player leaves.
    assert!(!s.iter().any(|(_, m)| m == "reverse"));
}

/// Readers `0x004C1550` and `0x008FFE80`: a trigger volume without bit 1 in
/// its flags is off until a message switches it on.
#[test]
fn a_trigger_is_off_at_load_without_its_flag() {
    let mut b = Builder::new();
    b.add(
        "TriggerBox",
        &[
            ("TriggerBase", 1, V::S("box_on")),
            ("TriggerBase", 4, V::S("touched")),
        ],
    );
    b.add(
        "EnterExitTrigger",
        &[
            ("EnterExitTrigger", 0, V::S("came_in")),
            ("EnterExitTrigger", 1, V::S("went_out")),
            ("EnterExitTrigger", 4, V::S("zone_on")),
        ],
    );
    b.add(
        "EnterExitTrigger",
        &[
            ("EnterExitTrigger", 0, V::S("came_in_2")),
            ("EnterExitTrigger", 1, V::S("went_out_2")),
            ("EnterExitTrigger", 6, V::I(1)),
        ],
    );
    let mut run = b.run();
    for (t, i) in parse_inputs(&[
        "enter:#0; enter:#1; enter:#2; exit:#2; send:box_on; send:zone_on; enter:#0; enter:#1; exit:#1"
            .into(),
    ])
    .unwrap()
    {
        run.schedule(t, i);
    }
    run.run(Until::Idle, 1000);
    assert_eq!(
        names(&run),
        [
            "came_in_2",
            "went_out_2",
            "box_on",
            "zone_on",
            "touched",
            "came_in",
            "went_out"
        ]
    );
}

/// `TriggerBase` attributes 9 and 6 (`0x004C1550`, `0x004C1260`): with
/// attribute 9 = 1 the message goes out when the touch ends; attribute 6
/// counts the sends and at 0 the object is gone.
#[test]
fn a_touch_sensor_fires_on_leave_and_counts_its_fires() {
    let mut b = Builder::new();
    b.add(
        "TriggerBox",
        &[
            ("TriggerBase", 4, V::S("left_the_box")),
            ("TriggerBase", 6, V::I(-1)),
            ("TriggerBase", 8, V::I(1)),
            ("TriggerBase", 9, V::I(1)),
        ],
    );
    b.add(
        "TriggerBox",
        &[
            ("TriggerBase", 4, V::S("once")),
            ("TriggerBase", 6, V::I(1)),
            ("TriggerBase", 8, V::I(1)),
        ],
    );
    let mut run = b.run();
    for (t, i) in
        parse_inputs(&["enter:#0; send:mark; exit:#0; enter:#1; enter:#1; enter:#1".into()])
            .unwrap()
    {
        run.schedule(t, i);
    }
    run.run(Until::Idle, 1000);
    assert_eq!(names(&run), ["mark", "left_the_box", "once"]);
}

/// Handler `0x0047AF60`, reader `0x0047B040`: the roll is taken over the
/// sum of all eight weights, and an entry without a message is passed over,
/// so its weight is the chance that nothing is sent.
#[test]
fn trigger_random_keeps_the_weight_of_an_empty_entry() {
    let mut b = Builder::new();
    b.add(
        "TriggerRandom",
        &[
            ("TriggerRandom", 0, V::S("roll")),
            ("TriggerRandom", 1, V::F(1.0)),
            ("TriggerRandom", 2, V::S("heads")),
            // Weight 3 without a message.
            ("TriggerRandom", 4, V::F(3.0)),
        ],
    );
    let mut run = b.run();
    let rolls = 400;
    for (t, i) in parse_inputs(&[vec!["send:roll"; rolls].join("; ")]).unwrap() {
        run.schedule(t, i);
    }
    run.run(Until::Idle, 100_000);
    let heads = names(&run).iter().filter(|m| *m == "heads").count();
    // A quarter of the rolls, not all of them.
    assert!((60..=140).contains(&heads), "{heads} of {rolls}");
}

/// Handler `0x00479B60`: the conditions are tried first and `reset` last,
/// and the comparison follows either.
#[test]
fn and_gate_tries_conditions_before_reset() {
    let mut b = Builder::new();
    // `both` is a condition and the reset: it counts as the condition.
    b.add(
        "AndEventGate",
        &[
            ("AndEventGate", 0, V::S("first")),
            ("AndEventGate", 1, V::S("both")),
            ("AndEventGate", 4, V::S("opened")),
            ("AndEventGate", 6, V::S("both")),
        ],
    );
    // No conditions at all: the gate sends on its reset.
    b.add(
        "AndEventGate",
        &[
            ("AndEventGate", 4, V::S("empty_gate")),
            ("AndEventGate", 6, V::S("clear")),
        ],
    );
    let mut run = b.run();
    for (t, i) in parse_inputs(&["send:first; send:both; send:clear".into()]).unwrap() {
        run.schedule(t, i);
    }
    run.run(Until::Idle, 1000);
    assert_eq!(
        names(&run),
        ["first", "both", "opened", "clear", "empty_gate"]
    );
}

/// `0x00477A90` tests the fire limit of a `VariableWatcher` on activation
/// only; the change callback `0x00477BE0` does not.
#[test]
fn a_watcher_tests_its_limit_only_when_activated() {
    let mut b = Builder::new();
    b.add(
        "VariableOperator",
        &[
            ("VariableOperator", 0, V::S("inc")),
            ("VariableOperator", 2, V::H("steps")),
            ("VariableOperator", 3, V::I(0)),
            ("VariableOperator", 4, V::I(1)),
        ],
    );
    b.add(
        "VariableWatcher",
        &[
            ("VariableWatcher", 0, V::S("watch")),
            ("VariableWatcher", 2, V::S("stepped")),
            ("VariableWatcher", 4, V::H("steps")),
            ("VariableWatcher", 5, V::I(4)),
            ("VariableWatcher", 6, V::I(0)),
            // Keeps watching (4); at most one fire.
            ("VariableWatcher", 7, V::I(4)),
            ("VariableWatcher", 8, V::I(1)),
        ],
    );
    let mut run = b.run();
    for (t, i) in
        parse_inputs(&["send:watch; send:inc; send:inc; send:watch; send:inc".into()]).unwrap()
    {
        run.schedule(t, i);
    }
    run.run(Until::Idle, 1000);
    // Past the limit the watch that is on goes on firing; activating again
    // neither stops it nor starts another.
    assert_eq!(names(&run).iter().filter(|m| *m == "stepped").count(), 3);
}

/// What the run models is what the code of the class was read to do: every
/// modelled class is described in `data/gf2/class_behavior.toml`, with the
/// messages the model reacts to named as its input attributes.
#[test]
fn modelled_classes_are_described_in_the_class_behaviour_data() {
    use crate::game_types::behavior::{Behaviors, UseKind};
    let all = Behaviors::gf2();
    let ins: &[(&str, &[(&str, u16)])] = &[
        ("MultiManager", &[("MultiManager", 0)]),
        ("MessageRelay", &[("MessageRelay", 0)]),
        (
            "AndEventGate",
            &[
                ("AndEventGate", 0),
                ("AndEventGate", 1),
                ("AndEventGate", 2),
                ("AndEventGate", 3),
                ("AndEventGate", 6),
            ],
        ),
        ("TriggerRandom", &[("TriggerRandom", 0)]),
        ("VariableOperator", &[("VariableOperator", 0)]),
        ("VariableSwitch", &[("VariableSwitch", 0)]),
        (
            "VariableWatcher",
            &[("VariableWatcher", 0), ("VariableWatcher", 1)],
        ),
        (
            "TriggerBase",
            &[("TriggerBase", 0), ("TriggerBase", 1), ("TriggerBase", 2)],
        ),
        (
            "EnterExitTrigger",
            &[("EnterExitTrigger", 4), ("EnterExitTrigger", 5)],
        ),
    ];
    for (class, attrs) in ins {
        let c = all
            .class(string_id(class))
            .unwrap_or_else(|| panic!("{class} is not described"));
        assert!(c.handler.is_some(), "{class}");
        for (packet, i) in *attrs {
            let a = all.attr(string_id(packet), *i).expect("attribute");
            assert_eq!(a.kind, UseKind::MsgIn, "{packet} {i}");
        }
    }
    for (packet, i) in [
        ("MultiManager", 3),
        ("TriggerAuto", 0),
        ("TriggerBase", 4),
        ("TriggerBase", 7),
        ("EnterExitTrigger", 0),
        ("EnterExitTrigger", 1),
        ("VariableWatcher", 2),
    ] {
        let a = all.attr(string_id(packet), i).expect("attribute");
        assert_eq!(a.kind, UseKind::MsgOut, "{packet} {i}");
    }
    // The handlers the models are read from.
    for (class, handler) in [
        ("MultiManager", "0x0047A1C0"),
        ("AndEventGate", "0x00479B60"),
        ("OrEventGate", "0x00479E30"),
        ("TriggerAuto", "0x00477EF0"),
        ("TriggerRandom", "0x0047AF60"),
        ("VariableOperator", "0x00476980"),
        ("VariableSwitch", "0x00476E50"),
        ("TriggerBase", "0x004C1260"),
        ("EnterExitTrigger", "0x00900E20"),
    ] {
        assert_eq!(
            all.class(string_id(class)).unwrap().handler.as_deref(),
            Some(handler),
            "{class}"
        );
    }
}

#[test]
fn gates_wait_for_all_or_any() {
    let mut b = Builder::new();
    b.add(
        "AndEventGate",
        &[
            ("AndEventGate", 0, V::S("key_a")),
            ("AndEventGate", 1, V::S("key_b")),
            ("AndEventGate", 4, V::S("both_keys")),
        ],
    );
    b.add(
        "OrEventGate",
        &[
            ("AndEventGate", 0, V::S("key_a")),
            ("AndEventGate", 1, V::S("key_b")),
            ("AndEventGate", 4, V::S("any_key")),
        ],
    );
    let mut run = b.run();
    for (t, i) in parse_inputs(&["send:key_a; send:key_a; wait:1; send:key_b".into()]).unwrap() {
        run.schedule(t, i);
    }
    run.run(Until::Idle, 1000);
    let s = sent(&run);
    assert_eq!(
        s.iter()
            .filter(|(_, m)| m == "both_keys")
            .collect::<Vec<_>>(),
        vec![&(1.0, "both_keys".to_owned())]
    );
    assert_eq!(s.iter().filter(|(_, m)| m == "any_key").count(), 3);
}

#[test]
fn lua_scripts_run_with_the_mock_api_and_join_the_bus() {
    let source = br#"
local function receive(self, message, dtype, data)
  local v = FindVariable("lua_count")
  VariableSet(v, VariableGet(v) + 1)
  SendMessage("lua_" .. message)
end
RegisterFunction("doctor_test.lua", "startup", function()
  r = NewMessageReceiver(receive)
  r:AddMessage("ping")
  t = CreateTimer("on_tick", true, false)
  StartTimer(t, 2000)
end)
function on_tick()
  SendMessage("tick_from_lua")
end
"#;
    let chunk = crate::resource::lua::compile_for_game(source, b"=doctor_test").unwrap();
    let mut b = Builder::new();
    b.add(
        "LuaAudioScript",
        &[
            ("LuaAudioScript", 1, V::S("start_lua")),
            ("LuaAudioScript", 3, V::G(Guid::from_resource_id(0x1234))),
        ],
    );
    mm(&mut b, "lua_ping", &[("heard_lua", 0.0)]);
    mm(&mut b, "tick_from_lua", &[("ping", 0.0)]);
    let mut input = b.input();
    input.chunks.insert(0x1234, chunk);
    let mut run = Run::new(input, RunOptions::default());
    for (t, i) in parse_inputs(&["send:start_lua; wait:1; send:ping".into()]).unwrap() {
        run.schedule(t, i);
    }
    assert_eq!(run.run(Until::Time(10.0), 1000), Stop::Time);
    let s = sent(&run);
    assert!(s.contains(&(1.0, "lua_ping".into())), "{s:?}");
    assert!(s.contains(&(1.0, "heard_lua".into())));
    // The timer fires 2 s after the start; its message pings the script again.
    assert!(s.contains(&(2.0, "tick_from_lua".into())));
    assert_eq!(s.iter().filter(|(_, m)| m == "lua_ping").count(), 2);
    assert!(run.variables().contains(&("lua_count".to_owned(), 2, true)));
}

#[test]
fn a_missing_script_is_a_warning() {
    let mut b = Builder::new();
    b.add(
        "LuaAudioScript",
        &[
            ("LuaAudioScript", 0, V::I(1)),
            ("LuaAudioScript", 3, V::G(Guid::from_resource_id(0x99))),
        ],
    );
    let mut run = b.run();
    run.schedule(0.0, Input::Load);
    run.run(Until::Idle, 100);
    assert!(
        run.journal()
            .iter()
            .any(|e| matches!(&e.what, What::Warning { text } if text.contains("not found")))
    );
}

#[test]
fn the_journal_compares_with_a_journal_of_the_game() {
    let mut b = Builder::new();
    let relay = b.add(
        "MessageRelay",
        &[
            ("MessageRelay", 0, V::S("a")),
            ("MessageRelay", 1, V::S("b")),
        ],
    );
    mm(&mut b, "b", &[("c", 0.0)]);
    let mut run = b.run();
    run.schedule(0.0, Input::Message("a".into()));
    run.run(Until::Idle, 100);
    let game = format!(
        r#"[
  {{"seq": 1, "sim_time_ms": 120000, "kind": "message", "hash": "{:08x}", "receivers": ["{relay}"]}},
  {{"seq": 2, "sim_time_ms": 120000, "kind": "message", "hash": {}}},
  {{"seq": 3, "sim_time_ms": 120500, "kind": "output", "text": "hello"}},
  {{"seq": 4, "sim_time_ms": 121000, "kind": "message", "message": "unrelated"}},
  {{"seq": 5, "sim_time_ms": 121000, "kind": "message", "message": "d"}}
]"#,
        string_id("a"),
        string_id("b"),
    );
    let messages = parse_game_journal(&game).unwrap();
    assert_eq!(messages.len(), 4);
    let mut scope = run.scope_messages();
    scope.insert(string_id("d"));
    let c = compare(run.journal(), &messages, &scope);
    assert_eq!(c.matched.len(), 2);
    assert_eq!(c.offset, Some(120.0));
    assert_eq!(c.only_run, vec![(c.only_run[0].0, "c".to_owned())]);
    assert_eq!(c.only_game, vec![(3, "d".to_owned())]);
    assert_eq!(c.ignored, 1);
    assert!(c.matched[0].only_game.is_empty() && c.matched[0].only_run.is_empty());

    // Our own export reads back as a journal.
    let ours = serde_json::to_string(run.journal()).unwrap();
    let back = parse_game_journal(&ours).unwrap();
    assert_eq!(back.len(), 3);
    let lines: String = run
        .journal()
        .iter()
        .map(|e| format!("{}\n", serde_json::to_string(e).unwrap()))
        .collect();
    assert_eq!(parse_game_journal(&lines).unwrap().len(), 3);
}

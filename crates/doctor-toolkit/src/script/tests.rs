//! Compiler tests: graphs are compiled, the chunk runs in the mock game
//! ([`super::sim`]) and the effects are checked.

use serde_json::json;

use super::compile::{Severity, compile};
use super::graph::{Node, PinType, ScriptGraph, Variable};
use super::sim::{Sim, SimEvent};

fn run(g: &ScriptGraph) -> Sim {
    let c = compile(g);
    assert!(!c.has_errors(), "{:#?}\n{}", c.diagnostics, c.source);
    let mut sim = Sim::new();
    sim.load(c.chunk.as_ref().unwrap()).unwrap();
    sim.start(&g.chunk_name()).unwrap();
    // On start runs at the first tick.
    sim.advance(0.001).unwrap();
    sim
}

fn node(kind: &str) -> Node {
    Node::new(0, kind, [0.0, 0.0])
}

#[test]
fn the_example_graphs_compile_and_run() {
    let text = include_str!("../../../../examples/scripts/doctor_e2e.json");
    let g = ScriptGraph::from_json(text).unwrap();
    let json = g.to_json();
    assert_eq!(ScriptGraph::from_json(&json).unwrap().to_json(), json);
    let mut sim = run(&g);
    sim.state
        .borrow_mut()
        .relays
        .push(("doctor_ping".into(), "doctor_pong".into()));
    sim.send("audio_init_delay").unwrap();
    sim.advance(5.0).unwrap();
    assert!(
        !sim.sent().contains(&"doctor_noon".to_owned()),
        "the player is not there yet"
    );
    sim.state.borrow_mut().player = [186.4, 21.2, -21.6];
    sim.advance(10.0).unwrap();
    let probes = sim.probes();
    for p in [
        "g_start",
        "g_var2_now_0",
        "g_audio_init_delay",
        "g_var2_later_0",
        "g_tick_3",
        "g_pos_186_-22",
        "g_spawned",
        "g_fade_sent",
    ] {
        assert!(
            probes.contains(&format!("doctor_probe_{p}.lua")),
            "{p}: {probes:?}"
        );
    }
    assert_eq!(
        sim.variable("doctor_var").and_then(|v| v.as_number()),
        Some(2.0)
    );
    let sent = sim.sent();
    assert!(sent.ends_with(&[
        "doctor_noon".into(),
        "doctor_fanfare".into(),
        "doctor_fade".into()
    ]));
}

#[test]
fn on_start_waits_for_the_first_tick_and_time_is_in_seconds() {
    let mut g = ScriptGraph::new("first_tick");
    let start = g.add(node("event.load"));
    let send = g.add(node("action.send_message"));
    let time = g.add(node("game.sim_time"));
    g.connect((start, "then"), (send, "exec"));
    g.connect((time, "seconds"), (send, "message"));
    let c = compile(&g);
    let mut sim = Sim::new();
    sim.load(c.chunk.as_ref().unwrap()).unwrap();
    sim.advance(2.0).unwrap();
    sim.start("first_tick.lua").unwrap();
    // A script started while its SimGroup loads must not send yet.
    assert!(sim.sent().is_empty());
    sim.advance(0.5).unwrap();
    assert_eq!(sim.sent(), ["2.0009999275208"]);
}

#[test]
fn hello_on_message_sends_a_message() {
    let mut g = ScriptGraph::new("hello");
    let ev = g.add(node("event.message").param("message", "iMsgPing"));
    let send = g.add(node("action.send_message").input("message", "doctor_pong"));
    g.connect((ev, "then"), (send, "exec"));
    let c = compile(&g);
    assert!(
        c.source.contains("SendMessage(\"doctor_pong\")"),
        "{}",
        c.source
    );
    assert!(c.source.contains(&format!("-- [{send}] Send message")));
    // Deterministic: the same graph gives the same source.
    assert_eq!(compile(&g).source, c.source);
    let mut sim = run(&g);
    assert!(sim.sent().is_empty());
    assert_eq!(sim.listened(), ["iMsgPing"]);
    sim.send("iMsgPing").unwrap();
    sim.send("iMsgOther").unwrap();
    sim.send("iMsgPing").unwrap();
    assert_eq!(sim.sent(), ["doctor_pong", "doctor_pong"]);
    sim.stop(&g.chunk_name()).unwrap();
    assert!(sim.listened().is_empty());
}

#[test]
fn branch_variables_and_math() {
    // count = count + 1 on every message; at 3 send "three", else "less".
    let mut g = ScriptGraph::new("counter");
    g.variables.push(Variable {
        name: "count".into(),
        ty: PinType::Number,
        default: json!(0),
    });
    let ev = g.add(node("event.message").param("message", "tick"));
    let get = g.add(node("var.get").param("variable", "count"));
    let add = g.add(node("math.add").input("b", 1));
    let set = g.add(node("var.set").param("variable", "count"));
    let eq = g.add(node("compare.ge").input("b", 3));
    let br = g.add(node("flow.branch"));
    let yes = g.add(node("action.send_message").input("message", "three"));
    let no = g.add(node("action.send_message"));
    let text = g.add(node("string.concat").input("a", "less_"));
    g.connect((get, "value"), (add, "a"));
    g.connect((add, "result"), (set, "value"));
    g.connect((ev, "then"), (set, "exec"));
    g.connect((set, "then"), (br, "exec"));
    g.connect((set, "value"), (eq, "a"));
    g.connect((eq, "result"), (br, "condition"));
    g.connect((br, "true"), (yes, "exec"));
    g.connect((br, "false"), (no, "exec"));
    g.connect((set, "value"), (text, "b"));
    g.connect((text, "result"), (no, "message"));
    let mut sim = run(&g);
    for _ in 0..4 {
        sim.send("tick").unwrap();
    }
    assert_eq!(sim.sent(), ["less_1", "less_2", "three", "three"]);
}

#[test]
fn timers_delays_and_sequence() {
    let mut g = ScriptGraph::new("timed");
    let t = g.add(
        node("event.timer")
            .param("name", "beat")
            .param("seconds", 2)
            .param("repeat", true),
    );
    let seq = g.add(node("flow.sequence").param("count", 2));
    let a = g.add(node("action.send_message").input("message", "beat"));
    let d = g.add(node("flow.delay").input("seconds", 0.5));
    let b = g.add(node("action.send_message").input("message", "after"));
    g.connect((t, "then"), (seq, "exec"));
    g.connect((seq, "then0"), (a, "exec"));
    g.connect((seq, "then1"), (d, "exec"));
    g.connect((d, "then"), (b, "exec"));
    // Stop the timer on a message.
    let ev = g.add(node("event.message").param("message", "halt"));
    let stop = g.add(node("timer.stop").param("timer", "beat"));
    g.connect((ev, "then"), (stop, "exec"));
    let mut sim = run(&g);
    sim.advance(1.0).unwrap();
    assert!(sim.sent().is_empty());
    sim.advance(1.6).unwrap(); // t = 2.6: beat at 2, after at 2.5
    assert_eq!(sim.sent(), ["beat", "after"]);
    sim.advance(2.0).unwrap(); // t = 4.6: beat at 4, after at 4.5
    assert_eq!(sim.sent().len(), 4);
    sim.send("halt").unwrap();
    sim.advance(10.0).unwrap();
    assert_eq!(sim.sent().len(), 4);
    let timers: Vec<_> = sim
        .take_log()
        .into_iter()
        .filter_map(|e| match e {
            SimEvent::Timer { time, .. } => Some(time),
            _ => None,
        })
        .collect();
    assert_eq!(timers, [2.0, 2.5, 4.0, 4.5]);
}

#[test]
fn delay_loop_repeats_and_gate_do_once_flip_flop() {
    // On start: send "ping", wait 1 s, loop back (a loop through a Delay is fine).
    let mut g = ScriptGraph::new("looper");
    let start = g.add(node("event.load"));
    let once = g.add(node("flow.do_once"));
    let ping = g.add(node("action.send_message").input("message", "ping"));
    let delay = g.add(node("flow.delay").input("seconds", 1));
    g.connect((start, "then"), (once, "exec"));
    g.connect((once, "then"), (ping, "exec"));
    g.connect((ping, "then"), (delay, "exec"));
    g.connect((delay, "then"), (ping, "exec"));
    // A gate closed by "close", messages "go" pass only while open; flip-flop alternates.
    let go = g.add(node("event.message").param("message", "go"));
    let gate = g.add(node("flow.gate"));
    let ff = g.add(node("flow.flip_flop"));
    let a = g.add(node("action.send_message").input("message", "A"));
    let b = g.add(node("action.send_message").input("message", "B"));
    let close = g.add(node("event.message").param("message", "close"));
    g.connect((go, "then"), (gate, "enter"));
    g.connect((gate, "exit"), (ff, "exec"));
    g.connect((ff, "a"), (a, "exec"));
    g.connect((ff, "b"), (b, "exec"));
    g.connect((close, "then"), (gate, "close"));
    let mut sim = run(&g);
    sim.advance(3.5).unwrap();
    assert_eq!(sim.sent(), ["ping"; 4]);
    sim.take_log();
    sim.send("go").unwrap();
    sim.send("go").unwrap();
    sim.send("go").unwrap();
    sim.send("close").unwrap();
    sim.send("go").unwrap();
    assert_eq!(sim.sent(), ["A", "B", "A"]);
}

#[test]
fn game_variables_positions_and_for_loops() {
    let mut g = ScriptGraph::new("vars");
    let start = g.add(node("event.load"));
    let get = g.add(node("engine.get").input("name", "doctor_var"));
    let add = g.add(node("math.add").input("b", 10));
    let set = g.add(node("engine.set").input("name", "doctor_var"));
    let lp = g.add(node("flow.for").input("first", 1).input("last", 3));
    let send = g.add(node("action.send_message"));
    let idx = g.add(node("string.from"));
    let pos = g.add(node("game.player_position"));
    let dist = g.add(
        node("game.distance_to")
            .input("x", 3)
            .input("y", 0)
            .input("z", 4),
    );
    let far = g.add(node("compare.gt").input("b", 4.5));
    let br = g.add(node("flow.branch"));
    let msg = g.add(node("action.send_message").input("message", "far"));
    g.connect((get, "value"), (add, "a"));
    g.connect((add, "result"), (set, "value"));
    g.connect((start, "then"), (set, "exec"));
    g.connect((set, "then"), (lp, "exec"));
    g.connect((lp, "body"), (send, "exec"));
    g.connect((lp, "index"), (idx, "value"));
    g.connect((idx, "result"), (send, "message"));
    g.connect((lp, "done"), (br, "exec"));
    g.connect((pos, "position"), (dist, "position"));
    g.connect((dist, "result"), (far, "a"));
    g.connect((far, "result"), (br, "condition"));
    g.connect((br, "true"), (msg, "exec"));
    let c = compile(&g);
    assert!(!c.has_errors(), "{:#?}", c.diagnostics);
    let mut sim = Sim::new();
    sim.state.borrow_mut().player = [0.0, 0.0, 0.0];
    sim.load(c.chunk.as_ref().unwrap()).unwrap();
    sim.start("vars.lua").unwrap();
    sim.advance(0.001).unwrap();
    assert_eq!(sim.sent(), ["1", "2", "3", "far"]);
    assert_eq!(
        sim.variable("doctor_var").and_then(|v| v.as_number()),
        Some(10.0)
    );
}

#[test]
fn functions_custom_lua_and_calls() {
    let mut g = ScriptGraph::new("funcs");
    let f = g.add(
        node("event.function")
            .param("name", "shout")
            .param("inputs", json!([{"name": "word", "type": "string"}])),
    );
    let code = g.add(
        node("lua.code")
            .param("code", "{loud} = string.upper({text}) .. \"!\"")
            .param("inputs", json!([{"name": "text", "type": "string"}]))
            .param("outputs", json!([{"name": "loud", "type": "string"}])),
    );
    let send = g.add(node("action.send_message"));
    g.connect((f, "then"), (code, "exec"));
    g.connect((f, "word"), (code, "text"));
    g.connect((code, "then"), (send, "exec"));
    g.connect((code, "loud"), (send, "message"));
    let start = g.add(node("event.load"));
    let call = g.add(
        node("flow.call_function")
            .param("function", "shout")
            .input("word", "hey"),
    );
    let lc = g.add(
        node("lua.call")
            .param("function", "PlayEmx")
            .param("args", 2)
            .input("arg1", "fanfare.emx")
            .input("arg2", 0.4),
    );
    g.connect((start, "then"), (call, "exec"));
    g.connect((call, "then"), (lc, "exec"));
    let sim = run(&g);
    assert_eq!(sim.sent(), ["HEY!"]);
    let calls: Vec<_> = sim
        .take_log()
        .into_iter()
        .filter_map(|e| match e {
            SimEvent::Call { function, args, .. } => Some((function, args)),
            _ => None,
        })
        .collect();
    assert_eq!(
        calls,
        [(
            "PlayEmx".to_owned(),
            vec!["fanfare.emx".to_owned(), "0.40000000596046".to_owned()]
        )]
    );
}

/// `PlayEmx` finds a sound only by its file name with `.emx` (stage 45, a
/// new emitter of `sound-add --new` in the game): the node adds it.
#[test]
fn play_sound_names_the_emx_file() {
    let mut g = ScriptGraph::new("beep");
    let start = g.add(node("event.load"));
    let plain = g.add(
        node("audio.play_emx")
            .input("sound", "doctor_beep")
            .input("volume", 1.0),
    );
    let full = g.add(
        node("audio.play_emx")
            .input("sound", "Fanfare.EMX")
            .input("volume", 1.0),
    );
    g.connect((start, "then"), (plain, "exec"));
    g.connect((plain, "then"), (full, "exec"));
    let sim = run(&g);
    let names: Vec<_> = sim
        .take_log()
        .into_iter()
        .filter_map(|e| match e {
            SimEvent::Call { function, args, .. } if function == "PlayEmx" => args.first().cloned(),
            _ => None,
        })
        .collect();
    assert_eq!(names, ["doctor_beep.emx", "Fanfare.EMX"]);
}

#[test]
fn errors_point_at_nodes() {
    let mut g = ScriptGraph::new("bad");
    let ev = g.add(node("event.message")); // empty message name
    let send = g.add(node("action.send_message").input("message", "x"));
    let send2 = g.add(node("action.send_message").input("message", "y"));
    g.connect((ev, "then"), (send, "exec"));
    g.connect((ev, "then"), (send2, "exec")); // two links from one exec output
    let num = g.add(node("value.string").param("value", "a"));
    let add = g.add(node("math.add"));
    g.connect((num, "value"), (add, "a")); // string into number
    let lonely = g.add(node("action.send_message")); // never runs
    let t = g.add(node("timer.start").param("timer", "nope"));
    let lua = g.add(node("lua.code").param("code", "local = 1"));
    g.connect((send, "then"), (t, "exec"));
    g.connect((t, "then"), (lua, "exec"));
    let c = compile(&g);
    let at = |id: u32, sev: Severity| c.for_node(id).any(|d| d.severity == sev);
    assert!(at(ev, Severity::Error), "{:#?}", c.diagnostics);
    assert!(at(add, Severity::Error));
    assert!(at(lonely, Severity::Warning));
    assert!(at(t, Severity::Error));
    assert!(c.chunk.is_none());

    // A synchronous loop is an error; Lua syntax errors map to their node.
    let mut g = ScriptGraph::new("loop");
    let start = g.add(node("event.load"));
    let a = g.add(node("action.send_message"));
    let b = g.add(node("action.send_message"));
    g.connect((start, "then"), (a, "exec"));
    g.connect((a, "then"), (b, "exec"));
    g.connect((b, "then"), (a, "exec"));
    let c = compile(&g);
    assert!(c.has_errors());
    let mut g = ScriptGraph::new("syntax");
    let start = g.add(node("event.load"));
    let lua = g.add(node("lua.code").param("code", "x = = 1"));
    g.connect((start, "then"), (lua, "exec"));
    let c = compile(&g);
    let e = c.errors().next().expect("an error");
    assert_eq!(e.node, Some(lua), "{e}");
    assert!(e.message.starts_with("Lua: line"), "{e}");
}

#[test]
fn every_node_kind_compiles() {
    // One node of every kind fed by an On start, with defaults: the source
    // must compile; runtime errors are caught by the prelude.
    let mut g = ScriptGraph::new("all");
    g.variables.push(Variable {
        name: "v".into(),
        ty: PinType::Any,
        default: json!(null),
    });
    let mut prev = (g.add(node("event.load")), "then".to_owned());
    for def in super::library::all() {
        let mut n = node(def.kind);
        match def.kind {
            "event.message" => n = n.param("message", "m"),
            "var.get" | "var.set" => n = n.param("variable", "v"),
            "lua.call" => n = n.param("function", "EclipseLog"),
            "flow.call_function" => n = n.param("function", "my_function"),
            "timer.start" | "timer.stop" => n = n.param("timer", "timer"),
            _ => {}
        }
        let id = g.add(n);
        let (ins, outs) = super::library::pins(&g, g.node(id).unwrap()).unwrap();
        for p in ins
            .iter()
            .filter(|p| matches!(p.ty, PinType::Position | PinType::Group))
        {
            let src = g.add(node(if p.ty == PinType::Position {
                "game.player_position"
            } else {
                "group.find"
            }));
            let out = if p.ty == PinType::Position {
                "position"
            } else {
                "group"
            };
            g.connect((src, out), (id, &p.name));
        }
        if def.is_pure() || def.is_event() || def.kind == "comment" {
            continue;
        }
        let entry = ins
            .iter()
            .find(|p| p.ty == PinType::Exec)
            .unwrap()
            .name
            .clone();
        g.connect((prev.0, &prev.1), (id, &entry));
        if let Some(next) = outs
            .iter()
            .find(|p| p.ty == PinType::Exec && p.name != "body")
        {
            prev = (id, next.name.clone());
        }
    }
    let c = compile(&g);
    assert!(!c.has_errors(), "{:#?}\n{}", c.diagnostics, c.source);
    let mut sim = Sim::new();
    sim.load(c.chunk.as_ref().unwrap()).unwrap();
    sim.start("all.lua").unwrap();
    sim.advance(5.0).unwrap();
}

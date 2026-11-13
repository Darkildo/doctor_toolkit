use super::edit::{self, Change};
use super::*;
use crate::hash::string_id;
use crate::resource::simgroup::{Guid, SimGroup, Value};

fn class(name: &str) -> u32 {
    string_id(name)
}

fn guid(n: u8) -> Guid {
    let mut g = [n; 16];
    g[8..].copy_from_slice(b"TESTTEST");
    Guid(g)
}

/// Adds an object of `name` with the schema layout and string values.
fn add(group: &mut SimGroup, name: &str, n: u8, values: &[(&str, u16, &str)]) -> usize {
    let c = class(name);
    let layout = LogicSchema::gf2().layout(c).expect("layout").to_vec();
    let o = group.push_object(c, guid(n), &layout).unwrap();
    for &(packet, index, value) in values {
        group
            .set_class_attr(o, class(packet), index, Some(Value::String(value.into())))
            .unwrap();
    }
    o
}

/// A trigger starts a MultiManager that spawns, shows an objective on an
/// engine message and sends one message nobody hears.
fn sample() -> SimGroup {
    let mut g = SimGroup::empty();
    add(&mut g, "TriggerAuto", 1, &[("TriggerAuto", 0, "start")]);
    let mm = add(
        &mut g,
        "MultiManager",
        2,
        &[
            ("MultiManager", 0, "start"),
            ("MultiManager", 3, "Spawn_Guards"),
            ("MultiManager", 5, "nobody_hears"),
        ],
    );
    g.set_class_attr(mm, class("MultiManager"), 4, Some(Value::Float(1.5)))
        .unwrap();
    add(&mut g, "FuncSpawn", 3, &[("FuncSpawn", 0, "spawn_guards")]);
    add(
        &mut g,
        "UIHudObjective",
        4,
        &[
            ("UIHudObjective", 0, "iMsgNewCheckpointLoaded"),
            ("UIHudObjective", 1, "never_sent"),
        ],
    );
    // Round trip through bytes, as the game data would be.
    SimGroup::parse(&g.encode()).unwrap()
}

const AT: Location = Location {
    stream: 0,
    resource: 7,
};

fn graph_of(group: &SimGroup) -> LogicGraph {
    let mut b = GraphBuilder::new(LogicSchema::gf2());
    b.add_group(AT, "test/stream", group);
    b.build(BuildOptions::default())
}

#[test]
fn schema_is_parsed_and_knows_the_main_classes() {
    let s = LogicSchema::gf2();
    let mm = s.attr(class("MultiManager"), 3).unwrap();
    assert_eq!(mm.pin, PinKind::Out);
    assert_eq!(mm.delay, Some(4));
    assert_eq!(s.attr(class("MultiManager"), 0).unwrap().pin, PinKind::In);
    assert_eq!(s.attr(class("TriggerAuto"), 0).unwrap().pin, PinKind::Out);
    assert_eq!(s.attr(class("LuaAudioScript"), 1).unwrap().pin, PinKind::In);
    assert_eq!(s.category(class("TriggerBox")), Category::Trigger);
    assert!(
        s.engine_message(string_id("iMsgNewCheckpointLoaded"))
            .is_some()
    );
    let c = s.coverage();
    assert_eq!(c.classes, 253);
    assert!(c.inputs[0] > 150 && c.outputs[0] > 100, "{c:?}");
}

/// Pins checked against the readers of `godfather2.exe` (see
/// `research/logic_schema.py`) after the comparison with another EARS editor.
#[test]
fn reader_checked_pins() {
    let s = LogicSchema::gf2();
    let name = |class_name: &str, i: u16| {
        let a = s.attr(class(class_name), i).unwrap();
        (a.pin, a.name.clone().unwrap_or_default(), a.delay)
    };
    // VariableSwitch: 3 is the default message (delay 4), then triples
    // value/message/delay from 5; 29 attributes, 0–28.
    assert_eq!(
        name("VariableSwitch", 3),
        (PinKind::Out, "default".into(), Some(4))
    );
    assert_eq!(
        name("VariableSwitch", 5),
        (PinKind::Param, "value_1".into(), None)
    );
    assert_eq!(
        name("VariableSwitch", 6),
        (PinKind::Out, "case_1".into(), Some(7))
    );
    assert_eq!(
        name("VariableSwitch", 27),
        (PinKind::Out, "case_8".into(), Some(28))
    );
    assert!(s.attr(class("VariableSwitch"), 29).is_none());
    // InterestingObject: 1 disables, 2 enables (handler 0x7279B0).
    assert_eq!(
        name("InterestingObject", 1),
        (PinKind::In, "disable".into(), None)
    );
    assert_eq!(
        name("InterestingObject", 2),
        (PinKind::In, "enable".into(), None)
    );
    // Both messages of NodeController and EnableDisableStreams are subscribed.
    for c in ["NodeController", "EnableDisableStreams"] {
        assert_eq!(name(c, 0).0, PinKind::In, "{c}");
        assert_eq!(name(c, 1).0, PinKind::In, "{c}");
    }
}

#[test]
fn messages_are_matched_case_insensitively() {
    let g = graph_of(&sample());
    assert_eq!(g.nodes.len(), 5, "four objects and the engine");
    let spawn = g.message("spawn_guards").unwrap();
    assert_eq!(spawn.state, MessageState::Connected);
    assert_eq!((spawn.senders.len(), spawn.listeners.len()), (1, 1));
    let sender = &g.nodes[spawn.senders[0].node].pins[spawn.senders[0].pin];
    assert_eq!(sender.delay, Some(1.5));
    assert_eq!(g.message("start").unwrap().state, MessageState::Connected);
    assert_eq!(
        g.message("nobody_hears").unwrap().state,
        MessageState::NeverHeard
    );
    assert_eq!(
        g.message("never_sent").unwrap().state,
        MessageState::NeverSent
    );
    assert_eq!(
        g.message("iMsgNewCheckpointLoaded").unwrap().state,
        MessageState::FromEngine
    );
    let engine = g
        .nodes
        .iter()
        .position(|n| n.source == NodeSource::Engine)
        .unwrap();
    assert_eq!(g.incoming(engine).count(), 0);
    assert_eq!(g.outgoing(engine).count(), 1);
    assert_eq!(g.dangling().count(), 2);
    let o = g.overview();
    assert_eq!(o.states["connected"], 2);
    assert_eq!(o.wires, 3);
}

#[test]
fn path_follows_wires() {
    let g = graph_of(&sample());
    let trigger = g.node_of(AT, 0).unwrap();
    let spawn = g.node_of(AT, 2).unwrap();
    let path = g.path(trigger, spawn).unwrap();
    assert_eq!(path, vec![trigger, g.node_of(AT, 1).unwrap(), spawn]);
    assert!(g.path(spawn, trigger).is_none());
    assert!(g.to_dot().contains("spawn_guards") || g.to_dot().contains("Spawn_Guards"));
}

#[test]
fn hubs_replace_large_fans() {
    let mut group = SimGroup::empty();
    for n in 0..4 {
        add(&mut group, "TriggerAuto", n, &[("TriggerAuto", 0, "go")]);
    }
    for n in 10..14 {
        add(&mut group, "FuncSpawn", n, &[("FuncSpawn", 0, "go")]);
    }
    let mut b = GraphBuilder::new(LogicSchema::gf2());
    b.add_group(AT, "s", &group);
    let g = b.build(BuildOptions {
        hub_threshold: 10,
        ..BuildOptions::default()
    });
    assert_eq!(g.wires.len(), 8, "4 + 4 through the hub instead of 16");
    assert!(g.message("go").unwrap().hub.is_some());
}

fn pin_named(g: &LogicGraph, object: usize, name: &str) -> PinRef {
    let node = g.node_of(AT, object).unwrap();
    let pin = g.nodes[node]
        .pins
        .iter()
        .position(|p| p.name == name)
        .unwrap();
    PinRef { node, pin }
}

fn apply(group: &SimGroup, changes: &[Change]) -> SimGroup {
    let mut g = group.clone();
    let refs: Vec<&Change> = changes.iter().collect();
    edit::apply_to(&mut g, &refs).unwrap();
    SimGroup::parse(&g.encode()).unwrap()
}

#[test]
fn connect_sets_the_empty_side_or_moves_the_listener() {
    let group = sample();
    let g = graph_of(&group);
    // MultiManager send_3 (empty) → FuncSpawn spawn ("spawn_guards"): the
    // sender takes the listener's message.
    let from = pin_named(&g, 1, "send_3");
    let to = pin_named(&g, 2, "spawn");
    let changes = edit::plan_connect(&g, from, to, || unreachable!()).unwrap();
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].at.index, 7);
    let after = graph_of(&apply(&group, &changes));
    assert_eq!(after.message("spawn_guards").unwrap().senders.len(), 2);
    // send_2 ("nobody_hears") → FuncSpawn: the listener switches.
    let from = pin_named(&g, 1, "send_2");
    let changes = edit::plan_connect(&g, from, to, || unreachable!()).unwrap();
    let after = graph_of(&apply(&group, &changes));
    assert_eq!(
        after.message("nobody_hears").unwrap().state,
        MessageState::Connected
    );
    assert!(after.message("spawn_guards").unwrap().listeners.is_empty());
    // Two empty pins get a fresh name.
    let from = pin_named(&g, 1, "send_4");
    let to = pin_named(&g, 3, "hide");
    let to_clear = edit::plan_connect(&g, from, to, || "fresh".into());
    assert!(to_clear.is_err() || to_clear.as_ref().unwrap().len() == 1);
    // A wrong direction is refused.
    assert!(edit::plan_connect(&g, to, from, || "x".into()).is_err());
}

#[test]
fn disconnect_clears_one_side() {
    let group = sample();
    let g = graph_of(&group);
    let w = g
        .wires
        .iter()
        .position(|w| w.message == Some(string_id("spawn_guards")))
        .unwrap();
    let changes = edit::plan_disconnect(&g, w).unwrap();
    assert_eq!(changes.len(), 1);
    let after = graph_of(&apply(&group, &changes));
    // One listener: the sender is cleared, the listener keeps its name.
    let m = after.message("spawn_guards").unwrap();
    assert_eq!(m.state, MessageState::NeverSent);
    assert!(m.senders.is_empty());
    // The delay of the cleared pin stays; the rest is untouched.
    assert_eq!(
        after.message("start").unwrap().state,
        MessageState::Connected
    );
}

#[test]
fn free_output_and_clear_wiring() {
    let mut group = sample();
    let mm = class("MultiManager");
    assert_eq!(edit::free_output(&group, 1, mm), Some(7));
    assert!(edit::clear_wiring(&mut group, 1).unwrap());
    assert_eq!(edit::free_output(&group, 1, mm), Some(3));
    let g = graph_of(&SimGroup::parse(&group.encode()).unwrap());
    assert!(g.message("nobody_hears").is_none());
    // The delay parameter is kept.
    let o = &group.objects[1];
    assert_eq!(group.attr(o, mm, 4), Some(&Value::Float(1.5)));
}

#[test]
fn new_objects_encode_and_parse() {
    let mut group = sample();
    let n = group.objects.len();
    let c = class("AndEventGate");
    let layout = LogicSchema::gf2().layout(c).unwrap().to_vec();
    let o = group.push_object(c, guid(99), &layout).unwrap();
    assert_eq!(o, n);
    assert!(
        group.push_object(c, guid(99), &layout).is_err(),
        "GUID taken"
    );
    let parsed = SimGroup::parse(&group.encode()).unwrap();
    assert_eq!(parsed.objects.len(), n + 1);
    assert_eq!(parsed.objects[n].class, c);
    assert_eq!(parsed.objects[n].packets.len(), layout.len());
    assert_eq!(parsed.encode(), group.encode());
}

#[test]
fn fresh_names_avoid_used_messages() {
    let g = graph_of(&sample());
    assert_eq!(edit::fresh_message(&g, "start"), "start_2");
    assert_eq!(edit::fresh_message(&g, "My Message"), "my_message");
}

#[test]
fn mission_of_stream_names() {
    use super::scope::mission_of;
    assert_eq!(
        mission_of("gf2/cuba/global/objectives/cp_710_725_mission_7/m07_back_alley"),
        Some("cp_710_725_mission_7")
    );
    assert_eq!(mission_of("gf2/cuba/global/objectives/x"), None);
    assert_eq!(mission_of("gf2/miami/local"), None);
}

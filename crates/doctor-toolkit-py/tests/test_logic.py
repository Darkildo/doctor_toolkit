"""The logic graph: schema, missions, graphs and messages as dicts."""

import pytest

import doctor_toolkit as dt

from conftest import corpus_dir


def test_schema_covers_the_classes():
    c = dt.logic_schema()
    assert c["classes"] == 253
    # [exe, data, other] counts per pin kind.
    assert c["inputs"][0] > 150
    assert c["outputs"][0] > 100


def test_sample_game_has_no_missions(sample):
    game, _, _ = sample
    assert dt.logic_missions(game) == []
    graph = dt.logic_graph(game, "game")
    assert graph["nodes"] == [] and graph["wires"] == []
    assert dt.logic_messages(game, "anything") is None
    with pytest.raises(dt.InvalidArgumentError):
        dt.logic_graph(game, "mission:nothing-like-this")


@pytest.mark.skipif(corpus_dir() is None, reason="needs EARS_GF2_DIR")
def test_mission_graph_and_messages():
    game = dt.Game(corpus_dir())
    names = [m["name"] for m in dt.logic_missions(game)]
    assert "chapter60-mission08-killingcastro" in names
    graph = dt.logic_graph(game, "mission:chapter60-mission08-killingcastro", original=True)
    assert len(graph["nodes"]) > 100
    assert graph["overview"]["states"]["connected"] > 50
    first = graph["wires"][0]
    assert {"from", "to", "kind", "message"} <= set(first)
    who = dt.logic_messages(game, "cp_737_loaded", original=True)
    assert who["senders"][0]["class_name"] == "Checkpoint"
    assert len(who["listeners"]) >= 3
    found = dt.logic_messages(game, "cp_737_", search=True, original=True)
    assert any(m["name"] == "cp_737_loaded" for m in found)


def test_sample_game_runs_an_empty_logic(sample):
    game, _, _ = sample
    r = dt.logic_run(game, "game", events=["load", "wait:1", "send:hello"])
    assert r["stop"]["stop"] == "idle"
    kinds = [e["kind"] for e in r["journal"]]
    assert kinds.count("input") == 2 and "send" in kinds
    with pytest.raises(dt.InvalidArgumentError):
        dt.logic_run(game, "game", events=["jump:high"])


@pytest.mark.skipif(corpus_dir() is None, reason="needs EARS_GF2_DIR")
def test_checkpoint_runs_without_the_game():
    game = dt.Game(corpus_dir())
    r = dt.logic_run(
        game,
        "checkpoint:737",
        events=["load", "send:cp_737_loaded", "send:cp_737_was_started"],
        until=30,
        original=True,
    )
    sent = [e["message"] for e in r["journal"] if e["kind"] == "send"]
    assert "cp_737_loaded_ag" in sent
    assert any(a["model"] == "black_box" for a in r["actors"])
    game_journal = '[{"kind": "message", "message": "cp_737_loaded", "sim_time_ms": 5000}]'
    again = dt.logic_run(
        game,
        "checkpoint:737",
        events=["load", "send:cp_737_loaded"],
        compare=game_journal,
        original=True,
    )
    assert len(again["comparison"]["matched"]) == 1


def test_messages_are_typed_without_a_game():
    engine = dt.message_info("imsgnewcheckpointloaded")
    assert engine["kind"] == "engine" and engine["name"] == "iMsgNewCheckpointLoaded"
    assert engine["usage"] == "heard_from_engine" and engine["conf"] == "exe"
    fade = dt.message_info("boundary_fade_out")
    assert fade["kind"] == "game" and fade["usage"] == "sent_and_heard"
    assert any(s["class"] == "TriggerBase" for s in fade["senders"])
    assert fade["data"] == "forward"
    assert dt.message_info("0x%08x" % fade["hash"])["name"] == "boundary_fade_out"
    custom = dt.message_info("my_mod_door_opened")
    assert custom["kind"] == "custom" and custom["collides_with"] is None
    assert dt.message_info("0xdeadbeef")["kind"] == "unknown"
    found = dt.message_search("cp_325_", limit=5)
    assert 0 < len(found) <= 5 and all("cp_325_" in m["name"] for m in found)


def test_value_lists_and_attribute_types():
    from doctor_toolkit import gf2_types

    family = dt.enum_values("family")
    names = [v["name"] for v in family["values"]]
    assert len(names) == 28 and "corleone" in names
    assert tuple(names) == gf2_types.FAMILY
    corleone = next(v for v in family["values"] if v["name"] == "corleone")
    assert corleone["hash"] == gf2_types.enum_hash("corleone") == dt.sdbm("corleone")
    assert [e["key"] for e in dt.enum_values()] == list(gf2_types.ENUMS)
    assert gf2_types.ENUMS["crew_specialty"]["values"][4]["value"] == 0x40
    assert "iMsgPostSpawnInitialize" in gf2_types.ENGINE_MESSAGES
    with pytest.raises(dt.InvalidArgumentError):
        dt.enum_values("planets")
    building = {a["index"]: a["type"] for a in dt.attr_types("Building")}
    assert building[3] == "Enum<city>" and building[4] == "Enum<family>"
    assert building[21] == "MsgIn"
    assert dt.attr_types("NoSuchClass") == []


def test_logic_check_without_a_workspace(sample):
    game, _, _ = sample
    report = dt.logic_check(game)
    assert report["ok"] is True

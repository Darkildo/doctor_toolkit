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

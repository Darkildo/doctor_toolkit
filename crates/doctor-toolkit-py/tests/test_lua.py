"""Lua scripts: compile, decompile, list."""

import pytest

import doctor_toolkit as dt

SOURCE = """local count = 0
function bump(n)
  if n and n > 0 then
    count = count + n
  end
  return count
end
"""


def test_compile_and_decompile_round_trip():
    chunk = dt.lua_compile(SOURCE, "x:\\scripts\\bump.lua")
    assert chunk[:5] == b"\x1bLua\x50"
    assert chunk[13] == 4  # float numbers, like the game
    info = dt.decode("lua", chunk)
    assert info["source"] == "x:\\scripts\\bump.lua"
    assert info["functions"] == 2
    out = dt.lua_decompile(chunk)
    assert out["round_trip"] == "identical"
    assert out["errors"] == []
    assert "function bump(n)" in out["source"]
    assert dt.lua_compile(out["source"], "x:\\scripts\\bump.lua") == chunk
    assert "GETGLOBAL" in dt.lua_listing(chunk) or "SETGLOBAL" in dt.lua_listing(chunk)


def test_double_numbers():
    chunk = dt.lua_compile("x = 0.1", number_size=8)
    assert chunk[13] == 8
    with pytest.raises(dt.InvalidArgumentError):
        dt.lua_compile("x = 1", number_size=2)


def test_syntax_error_has_the_line():
    with pytest.raises(dt.LuaSyntaxError) as info:
        dt.lua_compile("x = 1\ny = = 2\n")
    message, line = info.value.args
    assert line == 2
    assert "unexpected symbol" in message
    assert isinstance(info.value, dt.InvalidArgumentError)


def test_bad_chunk():
    with pytest.raises(dt.FormatError):
        dt.lua_decompile(b"not a chunk")


def test_script_compile():
    import json

    graph = {
        "format": "doctor-toolkit-script",
        "version": 1,
        "name": "relay",
        "nodes": [
            {"id": 1, "kind": "event.message", "params": {"message": "a"}},
            {"id": 2, "kind": "action.send_message", "inputs": {"message": "b"}},
        ],
        "links": [{"from": [1, "then"], "to": [2, "exec"]}],
    }
    out = dt.script_compile(json.dumps(graph))
    assert out["diagnostics"] == []
    assert 'SendMessage("b")' in out["source"]
    assert dt.decode("lua", out["chunk"])["source"] == "relay.lua"
    graph["nodes"][0]["params"]["message"] = ""
    bad = dt.script_compile(json.dumps(graph))
    assert bad["chunk"] is None
    assert bad["diagnostics"][0]["node"] == 1
    assert bad["diagnostics"][0]["severity"] == "error"

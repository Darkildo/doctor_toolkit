"""Editing a synthetic game: workspace, apply/unapply, mods, unpack/pack."""

import json
import struct
import zlib

import pytest

import doctor_toolkit as dt
from conftest import snapshot


def png(width, height, rgba):
    """A minimal RGBA PNG file."""

    def chunk(tag, data):
        body = tag + data
        return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body))

    rows = b"".join(b"\0" + rgba[y * width * 4 : (y + 1) * width * 4] for y in range(height))
    header = struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0)
    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", header)
        + chunk(b"IDAT", zlib.compress(rows))
        + chunk(b"IEND", b"")
    )


def test_workspace_records_without_touching_the_game(sample):
    game, keys, root = sample
    original = snapshot(root)
    ws = game.workspace()
    assert ws.dir == root / "_doctor_toolkit" / "workspace"
    assert len(ws) == 0

    assert ws.edit_strings(keys["strings"], {1: "Hi", 3: "Vito"})
    assert not ws.edit_strings(keys["strings"], {1: "Hi"})  # nothing changes
    assert keys["strings"] in ws
    assert ws.string_table(keys["strings"]).items() == [(1, "Hi"), (2, "World"), (3, "Vito")]
    assert ws.set_data(keys["vfx"], b"smoke\0", label="Smoke instead of fire")
    assert ws.keys == sorted([keys["strings"], keys["vfx"]])
    assert ws.read(keys["vfx"]) == b"smoke\0"
    assert ws.original_data(keys["vfx"]) == b"fire_big\0"
    assert ws.data(keys["text"]) is None
    assert ws.read(keys["text"]) == game.read(keys["text"])
    assert [c["label"] for c in ws.changes][1] == "Smoke instead of fire"
    assert snapshot(root) == original

    status = ws.status()
    assert status["fingerprint_matches"]
    assert not status["up_to_date"]
    assert dict(status["changes"]) == {keys["strings"]: "pending", keys["vfx"]: "pending"}

    diff = {d["key"]: d for d in ws.diff()}
    changed = diff[keys["strings"]]["strings"]["changed"]
    assert (1, "column 0", "Hello", "Hi") in [tuple(c) for c in changed]
    assert diff[keys["vfx"]]["original_size"] == 9 and diff[keys["vfx"]]["new_size"] == 6
    assert [d["key"] for d in ws.diff("vfx")] == [keys["vfx"]]

    assert [h["label"] for h in ws.history] == ["Edit strings in gf2/level", "Smoke instead of fire"]
    assert ws.undo() == "Smoke instead of fire"
    assert ws.redo_label == "Smoke instead of fire"
    assert ws.keys == [keys["strings"]]
    assert ws.redo() == "Smoke instead of fire"
    assert ws.undo_label == "Smoke instead of fire"

    # Reopening reads the same state from disk.
    again = game.workspace(create=False)
    assert again.keys == ws.keys
    with pytest.raises(dt.InvalidArgumentError, match="no workspace"):
        game.workspace(dir=root / "elsewhere", create=False)
    with pytest.raises(dt.InvalidArgumentError, match="already holds"):
        game.init_workspace()

    assert ws.revert(["vfx"])
    assert ws.keys == [keys["strings"]]
    with pytest.raises(KeyError):
        ws.revert(["no-such-change"])
    assert ws.revert_all()
    assert len(ws) == 0
    assert snapshot(root) == original


def test_apply_and_unapply(sample):
    game, keys, root = sample
    original = snapshot(root)
    ws = game.workspace()
    ws.edit_strings(keys["strings"], {3: "Michael"})
    report = ws.apply()
    assert report["streams"] == [("gf2/level", "rebuilt", 1)]
    assert "gf2/level.str" in report["written"]
    assert "gf2/level.toc" in report["written"]
    assert ws.status()["up_to_date"]
    assert dict(ws.status()["changes"]) == {keys["strings"]: "applied"}

    fresh = dt.Game(root)
    assert fresh.resource(keys["strings"]).string_table()[3] == "Michael"
    assert fresh.toc("gf2/level.toc")["files"][0]["data_size"] == fresh.stream("gf2/level").data_size
    assert snapshot(root) != original

    report = ws.unapply()
    assert report["streams"] == [("gf2/level", "restored", 0)]
    assert snapshot(root) == original
    assert dict(ws.status()["changes"]) == {keys["strings"]: "pending"}


def test_apply_refuses_streams_changed_by_another_program(sample):
    game, keys, root = sample
    ws = game.workspace()
    ws.set_data(keys["vfx"], b"smoke\0")
    ws.apply()
    # Another program rewrites the stream.
    path = root / "gf2/level.str"
    stream = path.read_bytes()
    path.write_bytes(stream[:-1] + bytes([stream[-1] ^ 1]))
    ws.set_data(keys["vfx"], b"steam\0")
    assert ws.status()["conflicts"] == ["gf2/level"]
    with pytest.raises(dt.DoctorToolkitError, match="changed"):
        ws.apply()
    ws.apply(force=True)
    assert dt.Game(root).read(keys["vfx"]) == b"steam\0"


def test_import_texture(sample):
    game, keys, root = sample
    ws = game.workspace()
    green = bytes([0, 255, 0, 255]) * 16
    texture = ws.import_texture(keys["texture_data"], png(4, 4, green))
    assert texture.name == "wall"
    assert texture.keys == (keys["texture_header"], keys["texture_data"])
    assert texture.rgba()[2] == green
    assert set(ws.keys) <= {keys["texture_header"], keys["texture_data"]}
    assert keys["texture_data"] in ws
    assert ws.history[-1]["label"] == "Import texture wall"
    texture_diff = [d for d in ws.diff() if "texture" in d]
    assert texture_diff and texture_diff[0]["texture"]["name"] == "wall"
    with pytest.raises(dt.InvalidArgumentError, match="PNG or DDS"):
        ws.import_texture(keys["texture_header"], b"not an image")
    ws.apply()
    assert dt.Game(root).resource(keys["texture_header"]).texture().rgba()[2] == green


def test_set_string_table_and_set_many(sample):
    game, keys, root = sample
    ws = game.workspace(dir=root.parent / "ws")
    table = game.resource(keys["text"]).string_table()
    table[1] = "Si"
    assert ws.set_string_table(table)
    assert ws.string_table(keys["text"])[1] == "Si"
    loose = dt.StringTable.parse(table.to_bytes())
    with pytest.raises(dt.InvalidArgumentError, match="pass `key`"):
        ws.set_string_table(loose)
    assert ws.set_string_table(loose, key=keys["text"]) is False
    assert ws.set_many({keys["vfx"]: b"a\0", keys["strings"]: game.read(keys["strings"])}) == 1
    assert ws.set_many({keys["vfx"]: b"b\0", keys["text"]: game.read(keys["text"])}) == 2
    assert keys["text"] not in ws  # back to the original
    with pytest.raises(ValueError, match="storage"):
        ws.set_data(keys["vfx"], b"c", storage="zip")
    assert ws.set_data(keys["vfx"], b"c", storage="refpack")
    assert ws.changes[-1]["storage"] == "refpack"


def test_mods_from_the_workspace(sample, tmp_path):
    game, keys, root = sample
    original = snapshot(root)
    ws = game.workspace()
    ws.edit_strings(keys["text"], {1: "Ja"})
    package = tmp_path / "yes.zip"
    built = ws.build_mod(package, "Yes Mod", author="tester", version="2.0")
    assert built["id"] == "yes-mod" and built["resources"] == 1
    assert built["added"] == 0 and built["new_streams"] == 0
    info = dt.mod_info(package)
    assert info["kind"] == "mod"
    # A mod that only replaces resources is still format version 1.
    assert info["format_version"] == 1 and "streams" not in info
    assert info["name"] == "Yes Mod" and info["version"] == "2.0"
    assert info["resources"][0]["key"] == keys["text"]

    mods = game.mods()
    assert mods.mods == []
    installed = mods.install(package)
    assert installed["id"] == "yes-mod" and installed["issues"] == []
    second = mods.install_workspace(ws, "Second")
    assert second["id"] == "second"
    assert [m["id"] for m in mods.mods] == ["yes-mod", "second"]
    conflicts = mods.conflicts()
    assert conflicts[0]["key"] == keys["text"] and conflicts[0]["same_data"]
    assert mods.clashes() == [] and mods.clashes(False) == []
    assert mods.mods[0]["added"] == 0 and mods.mods[0]["new_streams"] == []
    assert mods.check() == []
    mods.move("second", 0)
    mods.enable("second", False)
    assert [m["enabled"] for m in mods.mods] == [False, True]
    assert mods.manifest("yes-mod")["author"] == "tester"

    # The workspace and the mod manager never write into the game at the same time.
    ws.apply()
    with pytest.raises(dt.ModError, match="workspace"):
        mods.apply()
    ws.unapply()
    report = mods.apply()
    assert report["streams"] == [("gf2/text/abcd", "rebuilt", 1)]
    status = mods.status()
    assert dict(status["changes"]) == {keys["text"]: "applied"}
    assert status["streams"] == [("gf2/text/abcd", "applied", True)]
    assert dt.Game(root).resource(keys["text"]).string_table()[1] == "Ja"
    with pytest.raises(dt.ModError, match="mod manager"):
        ws.apply()
    mods.unapply()
    assert snapshot(root) == original
    mods.uninstall("second")
    assert [m["id"] for m in mods.mods] == ["yes-mod"]
    exported = tmp_path / "again.zip"
    assert mods.export("yes-mod", exported) > 0
    assert dt.mod_info(exported)["id"] == "yes-mod"
    bad = tmp_path / "bad.zip"
    bad.write_bytes(b"PK\x03\x04 broken")
    with pytest.raises(dt.ModError):
        dt.mod_info(bad)
    with pytest.raises(dt.IoError):
        dt.mod_info(tmp_path / "missing.zip")


def test_unpack_and_pack(sample, tmp_path):
    game, keys, root = sample
    out = tmp_path / "unpacked"
    report = game.unpack(out, streams=["gf2/level"])
    assert report["resources"] == 5 and report["failed"] == []
    manifest = json.loads((out / "manifest.json").read_text())
    files = {f["keys"][0]: f for f in manifest["files"] if f["kind"] == "raw"}
    vfx = out / files[keys["vfx"]]["path"]
    assert vfx.read_bytes() == b"fire_big\0"
    vfx.write_bytes(b"ember\0")
    dry = game.pack(out, dry_run=True)
    assert [c["keys"] for c in dry["changed"]] == [[keys["vfx"]]]
    packed = game.pack(out)
    assert packed["changed"][0]["recorded"]
    assert game.workspace().read(keys["vfx"]) == b"ember\0"
    index = game.index()
    only = game.unpack(tmp_path / "one", keys=[keys["strings"]], index=index)
    assert only["resources"] == 1
    with pytest.raises(dt.InvalidArgumentError, match="index"):
        game.unpack(tmp_path / "two", keys=[keys["strings"]])


def test_integrity_and_index_refresh(sample):
    game, keys, root = sample
    index = game.index()
    assert all(not f["changed"] for f in game.integrity())
    ws = game.workspace()
    ws.edit_strings(keys["strings"], {2: "Family"})
    ws.apply()
    changed = [f for f in game.integrity() if f["changed"]]
    assert {f["path"] for f in changed} == {"gf2/level.str", "gf2/level.toc"}
    index.refresh()
    assert index.report["rebuilt"] == 1
    assert [h["key"] for h in index.search("text:family")] == [keys["strings"]]


def bigf(entries):
    """A BIGF archive like the game's `.viv` files: entries aligned to 0x800."""
    table = b"".join(struct.pack(">II", 0, len(d)) + n.encode() + b"\0" for n, d in entries)
    header_size = 16 + len(table) + 8
    at = (header_size + 0x7FF) & ~0x7FF
    offsets = []
    for _, data in entries:
        offsets.append(at)
        at = (at + len(data) + 0x7FF) & ~0x7FF
    table = b"".join(
        struct.pack(">II", o, len(d)) + n.encode() + b"\0" for o, (n, d) in zip(offsets, entries)
    )
    out = bytearray(b"BIGF" + struct.pack("<I", 0) + struct.pack(">II", len(entries), header_size))
    out += table + b"L283\x15\x05\0\0"
    for o, (_, data) in zip(offsets, entries):
        out += b"\0" * (o - len(out)) + data
    out[4:8] = struct.pack("<I", len(out))
    return bytes(out)


def silent_snu(samples=128, rate=22050):
    """A mono EA-XAS `.snu` file of silence."""
    snr = struct.pack(">II", (4 << 24) | rate, (1 << 30) | samples)
    frames = (samples + 127) // 128
    block = struct.pack(">II", (0x80 << 24) | (8 + 76 * frames), samples) + b"\0" * 76 * frames
    head = bytes([3, 0, 0, 1]) + struct.pack("<III", 6, 0x20, 0)
    return head + snr + b"\0" * 8 + block


def wav(seconds, rate=44100):
    import math

    n = int(seconds * rate)
    pcm = b"".join(
        struct.pack("<h", int(8000 * math.sin(2 * math.pi * 440 * i / rate))) for i in range(n)
    )
    fmt = struct.pack("<HHIIHH", 1, 1, rate, rate * 2, 2, 16)
    body = b"WAVE" + b"fmt " + struct.pack("<I", 16) + fmt + b"data" + struct.pack("<I", len(pcm)) + pcm
    return b"RIFF" + struct.pack("<I", len(body)) + body


def test_streamed_sound_import_uses_an_overlay_archive(sample, tmp_path):
    game, _, root = sample
    entry = "audiostreams\\x\\music.exa.snu"
    (root / "GODFATH0.viv").write_bytes(bigf([(entry, silent_snu())]))
    original = snapshot(root)
    ws = game.workspace()
    assert ws.file_data("viv:" + entry) == silent_snu()
    source = tmp_path / "tone.wav"
    source.write_bytes(wav(0.5))
    seconds = ws.import_sound(entry, source)
    assert abs(seconds - 0.5) < 0.01
    assert ws.file_keys() == ["viv:" + entry]
    assert snapshot(root) == original
    ws.apply()
    # The overlay takes number 0; the game's archive moves up, unchanged.
    assert (root / "GODFATH1.viv").read_bytes() == bigf([(entry, silent_snu())])
    assert (root / "GODFATH0.viv").read_bytes()[:4] == b"BIGF"
    ws.unapply()
    assert snapshot(root) == original
    assert ws.revert_files(["viv:" + entry])
    assert ws.file_keys() == []

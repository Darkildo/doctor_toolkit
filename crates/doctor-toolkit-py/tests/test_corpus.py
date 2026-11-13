"""Tests on the installed game named by `EARS_GF2_DIR`; skipped without it.

The game directory is only read. Write tests work on copies of a few streams
in a temporary directory (`corpus_copy`), and check that the real files did
not change. The index cache of the full game goes to the user's cache
directory, as with the command line.
"""

import json

import pytest

import doctor_toolkit as dt

FRONTEND_TEXTURE = "gf2/frontend:tg4h:00000000.4802e0a7.f3d257db.9cf57f36"
DONS_VIEW = "gf2/cuba/global/dons_view/dons_view"


def first(game, tag, stream=None):
    streams = [stream] if stream else None
    return next(iter(game.resources(tag, streams=streams)))


def test_streams_and_resources(corpus):
    assert len(corpus.streams) == 2300
    assert len(corpus.tocs) == 25
    frontend = corpus.stream("gf2/frontend")
    assert len(frontend) > 40
    for resource in frontend:
        assert len(resource.read()) == resource.data_size
    assert corpus.resource(FRONTEND_TEXTURE).type == "tg4h"


def test_textures(corpus):
    textures = corpus.stream("gf2/frontend").textures()
    assert textures
    for texture in textures:
        width, height, rgba = texture.rgba()
        assert len(rgba) == width * height * 4
        assert texture.to_png().startswith(b"\x89PNG")
        assert texture.to_dds().startswith(b"DDS ")


def test_string_table(corpus):
    table = first(corpus, "lh2", "gf2/text/dd7d7050").string_table()
    assert len(table) > 1000
    assert dt.StringTable.parse(table.to_bytes()) == table


def test_logic_types(corpus):
    group = first(corpus, "simgroup", "gf2/cuba").parse()
    assert group["objects"]
    assert any("position" in o for o in group["objects"])
    json.dumps(group)
    meta = first(corpus, "metamodel", DONS_VIEW).parse()
    assert isinstance(meta, dict)
    assert isinstance(first(corpus, "material", DONS_VIEW).parse()["name"], str)
    assert first(corpus, "hkx", DONS_VIEW).parse()


def test_model(corpus):
    model = first(corpus, "mesh", DONS_VIEW).model()
    assert model.records
    vertices = model.vertices(0)
    assert len(vertices["positions"]) == vertices["count"] * 12
    triangles = model.triangles(0)
    assert len(triangles) % 12 == 0
    glb = model.to_glb()
    assert glb[:4] == b"glTF"


def test_model_with_assets(corpus):
    index = corpus.index()
    assert len(index) == 212_544
    model = first(corpus, "mesh", DONS_VIEW).model()
    with_assets = model.to_glb(index)
    assert with_assets[:4] == b"glTF"
    assert len(with_assets) > len(model.to_glb())
    hits = index.search("type:tg4h", limit=5)
    assert len(hits) == 5
    assert index.count("type:tg4h") == 28_246
    refs = index.refs(FRONTEND_TEXTURE)
    assert refs["key"] == FRONTEND_TEXTURE
    assert any(name == "tg4h" for name, _ in index.lookup(dt.sdbm("tg4h")))


def test_rig_font_sound_script(corpus):
    rig = first(corpus, "rcb").parse()
    assert rig["skeletons"] and rig["skeletons"][0]["bones"]
    font = first(corpus, "ffn", "gf2/frontend").parse()
    assert isinstance(font, dt.Font) and font.glyphs
    assert font.atlas is not None and font.atlas.to_png().startswith(b"\x89PNG")
    sounds = first(corpus, "sbk").parse()
    assert sounds and isinstance(sounds[0], dt.Sound)
    wav = sounds[0].to_wav()
    assert wav[:4] == b"RIFF" and wav[8:12] == b"WAVE"
    assert len(sounds[0].pcm()) == sounds[0].sample_count * sounds[0].channels * 2
    bank = dt.sound_bank(first(corpus, "sbk").read())
    assert bank["version"] == 206 and not bank["problems"]
    assert len(bank["samples"]) == len(sounds)
    emitter = bank["emitters"][0]
    assert emitter["instrument_index"] is not None and emitter["track"]["variations"]
    tones = bank["instruments"][emitter["instrument_index"]]["tones"]
    assert all(t["sample"] < len(sounds) for t in tones)
    script = first(corpus, "lua").parse()
    assert script["strings"]


def test_archived_sound(corpus):
    entries = corpus.archive_entries()
    assert len(entries) == 24_913
    sound = corpus.archive_sound(entries[0]["name"])
    assert sound.duration > 0
    assert sound.to_wav()[:4] == b"RIFF"


def test_workspace_on_copies(corpus_copy):
    game, root = corpus_copy
    ws = game.workspace()
    texture = game.resource(FRONTEND_TEXTURE).texture()
    width, height, rgba = texture.rgba()
    inverted = bytes(
        255 - b if i % 4 == 0 else b for i, b in enumerate(rgba)
    )
    import struct
    import zlib

    def chunk(tag, data):
        body = tag + data
        return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body))

    rows = b"".join(b"\0" + inverted[y * width * 4 : (y + 1) * width * 4] for y in range(height))
    png = (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0))
        + chunk(b"IDAT", zlib.compress(rows))
        + chunk(b"IEND", b"")
    )
    ws.import_texture(FRONTEND_TEXTURE, png)
    table = first(game, "lh2", "gf2/text/dd7d7050").string_table()
    first_hash = table.hashes[0]
    assert ws.edit_strings(table.key, {first_hash: "Edited from Python"})
    report = ws.apply()
    assert {s[0] for s in report["streams"]} == {"gf2/frontend", "gf2/text/dd7d7050"}
    fresh = dt.Game(root)
    assert fresh.resource(table.key).string_table()[first_hash] == "Edited from Python"
    assert fresh.resource(FRONTEND_TEXTURE).texture().format == texture.format
    ws.unapply()
    assert dt.Game(root).resource(table.key).string_table() == table


def test_import_model_on_copies(tmp_path, monkeypatch):
    """A model exported from a copy of a level stream imports back over itself."""
    import hashlib
    import shutil

    from conftest import corpus_dir

    path = corpus_dir()
    if path is None:
        pytest.skip("EARS_GF2_DIR is not set")
    monkeypatch.setenv("DOCTOR_TOOLKIT_CACHE_DIR", str(tmp_path / "cache"))
    copied = ["mp_rooftops/mp_rooftops.str", "mp_rooftops/mp_rooftops.toc"]
    before = {rel: hashlib.sha256((path / rel).read_bytes()).hexdigest() for rel in copied}
    root = tmp_path / "game"
    for rel in copied:
        (root / rel).parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(path / rel, root / rel)
    game = dt.Game(root)
    mesh = first(game, "mesh", "mp_rooftops/mp_rooftops")
    glb = tmp_path / "model.glb"
    glb.write_bytes(mesh.model().to_glb(game.index()))
    ws = game.init_workspace()
    plan = ws.import_model(mesh.key, glb, dry_run=True)
    assert plan["mode"] == "replace"
    assert plan["parts"] and plan["materials_new"] == 0
    summary = ws.import_model(mesh.key, glb)
    assert summary["meshes"] == [mesh.key]
    assert len(ws.history) == 1
    ws.apply()
    assert dt.Game(root).resource(mesh.key).model().records
    after = {rel: hashlib.sha256((path / rel).read_bytes()).hexdigest() for rel in copied}
    assert after == before, "the real game files changed"


def test_export_model_of_the_workspace_version(tmp_path, monkeypatch):
    """`Workspace.export_model` gives the GLB that imports back over the model
    (the Blender add-on's round trip), before and after an import."""
    import shutil

    from conftest import corpus_dir

    path = corpus_dir()
    if path is None:
        pytest.skip("EARS_GF2_DIR is not set")
    monkeypatch.setenv("DOCTOR_TOOLKIT_CACHE_DIR", str(tmp_path / "cache"))
    root = tmp_path / "game"
    for rel in ["mp_rooftops/mp_rooftops.str", "mp_rooftops/mp_rooftops.toc"]:
        (root / rel).parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(path / rel, root / rel)
    game = dt.Game(root)
    index = game.index()
    mesh = first(game, "mesh", "mp_rooftops/mp_rooftops")
    ws = game.init_workspace()
    glb = ws.export_model(mesh.key, index)
    assert glb[:4] == b"glTF"
    assert glb == ws.export_model(mesh.key)  # the same without the index
    path_glb = tmp_path / "model.glb"
    path_glb.write_bytes(glb)
    summary = ws.import_model(mesh.key, path_glb)
    assert summary["materials_new"] == 0
    again = ws.export_model(mesh.key, index)
    assert again[:4] == b"glTF"
    # Metamodels export their first mesh.
    meta = first(game, "metamodel", "mp_rooftops/mp_rooftops")
    try:
        assert ws.export_model(meta.key, index)[:4] == b"glTF"
    except dt.InvalidArgumentError as error:
        assert "no mesh" in str(error)
    with pytest.raises(dt.InvalidArgumentError):
        ws.export_model(first(game, "tg4h", "mp_rooftops/mp_rooftops").key)

def test_import_animation_on_copies(tmp_path, monkeypatch):
    """The `run` clip exported from a copy of globals_global replaces `idle`."""
    import shutil

    from conftest import corpus_dir

    path = corpus_dir()
    if path is None:
        pytest.skip("EARS_GF2_DIR is not set")
    monkeypatch.setenv("DOCTOR_TOOLKIT_CACHE_DIR", str(tmp_path / "cache"))
    root = tmp_path / "game"
    for rel in ["gf2/globals_global.str", "gf2/globals_global.toc"]:
        (root / rel).parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(path / rel, root / rel)
    game = dt.Game(root)
    bank = game.stream("gf2/globals_global")[2657]
    glb = tmp_path / "run.glb"
    glb.write_bytes(dt.animation_glb(game, "gf2/globals_global", 2657, clips=["run"]))
    ws = game.init_workspace()
    plan = ws.import_animation(bank.key, glb, "idle", dry_run=True)
    assert plan["mode"] == "replace" and plan["frames"] == 19 and not plan["unmapped"]
    assert not ws.history
    ws.import_animation(bank.key, glb, "my_run", add=True)
    clips = dt.animation_bank(ws.read(bank.key))["clips"]
    assert len(clips) == 303 and clips[-1]["name"] == "my_run"
    events = [{"frame": 4.0, "name": "footstep", "signature": 0, "args": ["footstep_male_run_toe"]}]
    assert ws.set_clip_events(bank.key, "my_run", events)
    clips = dt.animation_bank(ws.read(bank.key))["clips"]
    assert clips[-1]["events"][0]["args"] == ["footstep_male_run_toe"]
    assert len(ws.history) == 2
    ws.undo()
    assert dt.animation_bank(ws.read(bank.key))["clips"][-1]["events"] == []
    # The node graph: the entry `idle` plays ten clips through random nodes.
    graph = dt.animation_graph(ws.read(bank.key))
    assert graph["slots"] == 118 and not [i for i in graph["issues"] if i["severity"] == "error"]
    idle = next(e for e in graph["entries"] if e["name"] == "idle")
    assert idle["root"]["kind"] == 2
    # Raise the weight of the first child of the first random node.
    def walk(node):
        yield node
        for child in node.get("children", []):
            yield from walk(child)
    random = next(n for n in walk(idle["root"]) if n["kind"] == 3)
    random["children"][0]["extra"] = [9.0]
    assert ws.set_animation_graph(bank.key, {"entries": graph["entries"]})
    again = dt.animation_graph(ws.read(bank.key))
    idle = next(e for e in again["entries"] if e["name"] == "idle")
    assert next(n for n in walk(idle["root"]) if n["kind"] == 3)["children"][0]["extra"] == [9.0]
    broken = {"entries": [{"name": "x", "root": {"kind": 0x16, "params": [0, 1.0, 2.0], "clip": 9999}}]}
    with pytest.raises(Exception, match="not in the bank"):
        ws.set_animation_graph(bank.key, broken)


ALLCHARACTERS = "gf2/globalfolder/characters/allcharacters"


def test_parts_database_character_and_animation(corpus):
    player = next(
        r for r in corpus.stream(ALLCHARACTERS).resources("partsdb")
        if r.parse()["name"] == "player"
    )
    db = player.parse()
    assert len(db["sets"]) == 48 and db["kind"] == 2
    glb = dt.character_glb(corpus, ALLCHARACTERS, "player", "_all")
    assert glb[:4] == b"glTF"
    bank = corpus.stream("gf2/globals_global")[2657]
    clips = dt.animation_bank(bank.read())["clips"]
    assert len(clips) == 302
    anim = dt.animation_glb(corpus, "gf2/globals_global", 2657, clips=["idle"])
    header = json.loads(anim[20:20 + int.from_bytes(anim[12:16], "little")])
    assert header["animations"][0]["name"] == "idle"
    fbx = dt.animation_glb(corpus, "gf2/globals_global", 2657, clips=["idle"], format="fbx")
    assert fbx.startswith(b"Kaydara FBX Binary")
    usdz = dt.character_glb(corpus, ALLCHARACTERS, "player", "_all", format="usdz")
    assert usdz[:2] == b"PK"
    with pytest.raises(dt.InvalidArgumentError):
        dt.character_glb(corpus, ALLCHARACTERS, "player", "_all", format="obj")


def test_model_formats(corpus, tmp_path):
    """A model written in every format reads back with the same parts."""
    model = first(corpus, "mesh", "gf2/globals_global").model()
    index = corpus.index()
    counts = None
    for ext in ["glb", "fbx", "obj", "usd", "usda", "usdc", "usdz"]:
        written = model.export(tmp_path / f"m.{ext}", index)
        assert str(written[0]).endswith(f"m.{ext}")
        info = dt.read_model(written[0])
        parts = [(p["vertices"], p["triangles"]) for p in info["parts"]]
        if counts is None:
            counts = parts
        assert [t for _, t in parts] == [t for _, t in counts], ext
    obj = dt.read_model(tmp_path / "m.obj")
    assert any("skeleton" in w for w in obj["warnings"])


def test_navigation_graph(corpus):
    stream = "gf2/miami/local/betsy/high/terrain1"
    data = next(r for r in corpus.resources("graph", streams=[stream]) if r.data_size > 1000).read()
    graph = dt.navigation_graph(data)
    assert graph["version"] == 16 and graph["info"]["nodes"] == len(graph["nodes"]) == 81
    assert dt.navigation_graph_bytes(graph) == data
    # Moving a node: the lengths of its links follow.
    graph["nodes"][0]["position"][0] += 2.0
    moved = dt.navigation_graph(dt.navigation_graph_bytes(graph))
    assert moved["info"]["stale_lengths"] == 0
    assert moved["nodes"][0]["links"][0]["length"] != graph["nodes"][0]["links"][0]["length"]

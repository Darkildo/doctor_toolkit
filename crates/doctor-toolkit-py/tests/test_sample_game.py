"""Reading a synthetic game: hashes, streams, resources and parsed views."""

import json

import pytest

import doctor_toolkit as dt


def test_hash_helpers():
    assert dt.sdbm("tg4h") == 0x4802E0A7
    assert dt.sdbm(b"tg4h") == 0x4802E0A7
    assert dt.crc32(b"123456789") == 0xCBF43926
    assert dt.fnv1a("") == 0x811C9DC5
    hashes = dt.hash_all("Entity")
    assert hashes["sdbm(lower)"] == 0x38523FC3
    assert set(hashes) == {"sdbm", "sdbm(lower)", "crc32", "crc32(lower)", "fnv1a", "fnv1a(lower)"}
    assert dt.type_id("tg4h") == 0x4802E0A7
    assert dt.type_id("0xcdb9d42b") == 0xCDB9D42B
    assert dt.type_id(0x1234) == 0x1234
    assert dt.type_id("newtype") == dt.sdbm("newtype")
    assert dt.type_name(0x4802E0A7) == "tg4h"
    assert dt.type_name(0xDEADBEEF) == "deadbeef"
    assert ("Entity", "sdbm(lower)") in dt.lookup(0x38523FC3)
    with pytest.raises(TypeError):
        dt.sdbm(12)


def test_keys():
    parts = dt.parse_key("gf2/frontend:tg4h:00000000.4802e0a7.f3d257db.9cf57f36")
    assert parts == {
        "stream": "gf2/frontend",
        "type": "tg4h",
        "type_id": 0x4802E0A7,
        "ids": [0, 0x4802E0A7, 0xF3D257DB, 0x9CF57F36],
    }
    assert dt.parse_key("GF2\\Text\\X.str:lh2:00000002")["ids"] == [2, 2, 2, 2]
    with pytest.raises(dt.InvalidArgumentError, match="bad resource key"):
        dt.parse_key("nonsense")


def test_game_and_streams(sample):
    game, keys, root = sample
    assert game.streams == ["gf2/level", "gf2/text/abcd"]
    assert game.tocs == ["gf2/level.toc"]
    assert game.profile == "The Godfather II (PC)"
    assert str(game.root) == str(root)
    files = game.stream_files()
    assert files[0]["path"] == "gf2/level.str" and files[0]["size"] > 0
    assert game.has_stream("GF2\\Level.str")
    with pytest.raises(KeyError):
        game.stream("gf2/missing")
    toc = game.toc("gf2/level.toc")
    assert [f["name"] for f in toc["files"]] == ["gf2\\level", "gf2\\text\\abcd"]
    assert toc["files"][0]["data_size"] == game.stream("gf2/level").data_size

    stream = game.stream("gf2/level")
    assert stream.name == "gf2/level"
    assert stream.rel_path == "gf2/level.str"
    assert len(stream) == 5
    assert [r.type for r in stream] == ["tg4h", "tg4d", "lh2", "simgroup", "vfx"]
    assert stream[-1].key == keys["vfx"]
    with pytest.raises(IndexError):
        stream[5]
    assert [r.index for r in stream.resources("lh2")] == [2]
    assert stream.find("vfx", 0x30).index == 4
    assert stream.find("vfx", 0x31) is None
    assert stream.get(keys["strings"]).index == 2
    assert stream.get(keys["text"]) is None
    assert stream.read(4) == b"fire_big\0"


def test_resources(sample):
    game, keys, _ = sample
    resource = game.resource(keys["texture_data"])
    assert resource.stream == "gf2/level"
    assert resource.index == 1
    assert resource.type == "tg4d"
    assert resource.type_id == dt.sdbm("tg4d")
    assert resource.ids == [0, dt.sdbm("tg4d"), 7, 0x55]
    assert resource.id == 0x55
    assert resource.compressed
    assert resource.data_size == 8 == len(resource.read())
    assert resource.mem_policy == "default"
    info = resource.to_dict()
    assert info["key"] == keys["texture_data"] and info["compressed"] is True
    assert game.read(keys["vfx"]) == b"fire_big\0"
    assert game.read(resource) == resource.read()
    with pytest.raises(KeyError):
        game.resource("gf2/level:lh2:00000077")

    everything = list(game.resources())
    assert len(everything) == 6
    tables = [r.key for r in game.resources("lh2")]
    assert tables == [keys["strings"], keys["text"]]
    assert [r.key for r in game.resources(streams=["gf2/text/abcd"])] == [keys["text"]]


def test_texture(sample):
    game, keys, _ = sample
    texture = game.resource(keys["texture_header"]).texture()
    assert texture.name == "wall"
    assert texture.package == "sample_pkg"
    assert (texture.width, texture.height) == (4, 4)
    assert texture.format == "DXT1c"
    assert texture.kind == "flat"
    assert texture.mip_count == 1 and texture.layer_count == 1
    assert texture.keys == (keys["texture_header"], keys["texture_data"])
    assert game.resource(keys["texture_data"]).texture().name == "wall"
    width, height, rgba = texture.rgba()
    assert (width, height) == (4, 4)
    assert rgba == bytes([255, 0, 0, 255]) * 16
    png = texture.to_png()
    assert png.startswith(b"\x89PNG")
    assert texture.to_dds().startswith(b"DDS ")
    assert texture.to_dict()["format"] == "DXT1c"
    assert len(game.stream("gf2/level").textures()) == 1
    # A replacement from its own PNG gives the same pixels.
    again = texture.replaced_by(png)
    assert again.rgba()[2] == rgba
    with pytest.raises(dt.InvalidArgumentError):
        texture.replaced_by(b"GIF89a")
    with pytest.raises(dt.InvalidArgumentError, match="not a texture"):
        game.resource(keys["vfx"]).texture()


def test_string_table(sample):
    game, keys, _ = sample
    table = game.resource(keys["strings"]).string_table()
    assert table.key == keys["strings"]
    assert table.items() == [(1, "Hello"), (2, "World"), (3, "Don Corleone")]
    assert len(table) == 3 and 2 in table and 9 not in table
    assert table[1] == "Hello"
    assert table.get(9) is None
    table[1] = "Hi"
    table.set(2, "Earth", language=0)
    assert table.items()[:2] == [(1, "Hi"), (2, "Earth")]
    with pytest.raises(KeyError):
        table[9] = "x"
    with pytest.raises(IndexError):
        table.get(1, language=3)
    copy = dt.StringTable.parse(table.to_bytes())
    assert copy == table
    assert copy.key is None
    assert table.to_dict()["strings"][3] == ["Don Corleone"]
    with pytest.raises(dt.FormatError):
        dt.StringTable.parse(b"not a table")
    assert isinstance(game.resource(keys["strings"]).parse(), dt.StringTable)


def test_simgroup(sample):
    game, keys, _ = sample
    group = game.resource(keys["simgroup"]).parse()
    assert group["group_hash"] == "deadbeef"
    objects = {o["name"]: o for o in group["objects"]}
    assert set(objects) == {"don_vito", "barrel_01"}
    assert objects["don_vito"]["class_name"] == "Entity"
    assert objects["don_vito"]["position"] == [1.0, 2.0, 3.0]
    assert objects["barrel_01"]["position"] == [-4.0, 0.5, 10.0]
    assert objects["barrel_01"]["model"] == f"{keys['model_id']:08x}"
    json.dumps(group)  # plain Python data
    # The same through `decode` on raw bytes.
    assert dt.decode("simgroup", game.read(keys["simgroup"])) == group


def test_unparsed_types_and_errors(sample, tmp_path):
    game, keys, _ = sample
    with pytest.raises(dt.InvalidArgumentError, match="no parser"):
        game.resource(keys["vfx"]).parse()
    with pytest.raises(dt.FormatError):
        dt.decode("simgroup", b"\0" * 64)
    assert issubclass(dt.FormatError, dt.DoctorToolkitError)
    assert issubclass(dt.ModError, dt.DoctorToolkitError)
    assert issubclass(dt.IoError, dt.DoctorToolkitError)
    with pytest.raises(dt.IoError, match="No such file"):
        dt.Game("/nonexistent/doctor-toolkit-game")
    (tmp_path / "empty").mkdir()
    with pytest.raises(dt.InvalidArgumentError, match="not a game directory"):
        dt.Game(str(tmp_path / "empty"))
    with pytest.raises(dt.InvalidArgumentError, match="already holds"):
        dt.testing.build_sample_game(game.root)


def test_index(sample):
    game, keys, _ = sample
    index = game.index()
    assert len(index) == 6
    assert index.stream_count == 2
    assert index.cache_file is not None and index.cache_file.exists()
    assert index.report["rebuilt"] == 2
    hits = index.search("don_vito")
    assert [h["key"] for h in hits] == [keys["simgroup"]]
    assert any("don_vito" in h for h in hits[0]["hits"])
    assert [h["key"] for h in index.search("text:corleone")] == [keys["strings"]]
    assert [h["key"] for h in index.search("type:lh2")] == [keys["strings"], keys["text"]]
    assert index.count("type:lh2") == 2
    assert [h["key"] for h in index.search(keys["vfx"])] == [keys["vfx"]]
    with pytest.raises(dt.InvalidArgumentError):
        index.search("type:nonsense_type_zz")
    record = index.record(keys["simgroup"])
    assert ("object", "don_vito") in [tuple(n) for n in record["names"]]
    texts = index.record(keys["strings"])["texts"]
    assert (3, "text", "Don Corleone") in [tuple(t) for t in texts] or any(
        t[2] == "Don Corleone" for t in texts
    )
    refs = index.refs(keys["texture_header"])
    assert set(refs) >= {"uses", "used_by", "copies", "key", "label"}
    assert index.keys("lh2") == [keys["strings"], keys["text"]]
    assert index.keys(stream="gf2/text/abcd") == [keys["text"]]
    assert ("wall", "stored in tg4h") in index.lookup(0x55) or any(
        name == "wall" for name, _ in index.lookup(0x55)
    )
    # A second open reuses the cache.
    again = game.index()
    assert again.report["reused"] == 2 and again.report["rebuilt"] == 0
    assert len(game.index(cache=False)) == 6


def test_threads(sample):
    """Methods that release the GIL work from several threads at once."""
    from concurrent.futures import ThreadPoolExecutor

    game, keys, _ = sample
    texture = game.resource(keys["texture_header"]).texture()
    index = game.index()

    def work(_):
        return (
            texture.to_png(),
            game.read(keys["strings"]),
            len(index.search("type:lh2")),
            game.resource(keys["simgroup"]).parse()["group_hash"],
        )

    with ThreadPoolExecutor(4) as pool:
        results = list(pool.map(work, range(16)))
    assert all(r == results[0] for r in results)

"""Projects on a synthetic game: make, status, build, mod (docs/project.md)."""

import json

import pytest

import doctor_toolkit as dt
from conftest import snapshot


def test_project_builds_changed_files_into_the_workspace(sample, tmp_path):
    game, keys, root = sample
    original = snapshot(root)
    project = dt.Project.create(game, tmp_path / "mod", name="Sample Mod", streams=["gf2/level"])
    assert project.id == "sample-mod"
    entries = {e["keys"][0]: e for e in project.entries()}
    assert entries[keys["texture_header"]]["kind"] == "texture"
    assert entries[keys["strings"]]["kind"] == "strings"
    assert entries[keys["vfx"]]["kind"] == "raw"
    assert project.status()["to_build"] == 0
    assert project.build()["built"] == []

    # Edit the strings and the raw data of the effect.
    strings = project.root / entries[keys["strings"]]["files"][0]
    doc = json.loads(strings.read_text())
    doc["entries"][0]["text"] = "Hi"
    strings.write_text(json.dumps(doc))
    (project.root / entries[keys["vfx"]]["files"][0]).write_bytes(b"ember\0")
    states = {e["id"]: e["state"] for e in project.status()["entries"]}
    assert sorted(s for s in states.values() if s != "unchanged") == ["modified", "modified"]

    report = project.build()
    assert report["errors"] == [] and len(report["built"]) == 2
    assert report["seq"] is not None
    ws = game.workspace()
    assert ws.read(keys["vfx"]) == b"ember\0"
    assert ws.string_table(keys["strings"]).items()[0][1] == "Hi"
    assert project.build()["built"] == []

    # The project opens again; a mod holds its two changes.
    again = dt.Project(project.root, game=game)
    assert again.status()["to_build"] == 0
    info = again.build_mod(tmp_path / "mod.zip")
    assert info["resources"] == 2
    assert dt.mod_info(tmp_path / "mod.zip")["id"] == "sample-mod"
    assert snapshot(root) == original

    # A broken file is an error with its path; the rest still builds.
    strings.write_text("{")
    report = project.build()
    assert len(report["errors"]) == 1 and report["errors"][0].startswith(
        entries[keys["strings"]]["files"][0]
    )


def test_project_object_guids_come_from_names():
    a = dt.project_object_guid("mod:simgroup:00000004", "lamp")
    assert a == dt.project_object_guid("mod:simgroup:00000004", "lamp")
    assert a != dt.project_object_guid("mod:simgroup:00000004", "lamp2")
    # The canonical GUID of the tool: machine name `XDOCTORTKX`.
    assert bytes.fromhex(a)[8:16] == b"DOCTORTK"
    assert bytes.fromhex(a)[3:4] == b"X"


def test_a_project_folder_must_be_new(sample, tmp_path):
    game, _, _ = sample
    folder = tmp_path / "busy"
    folder.mkdir()
    (folder / "file.txt").write_text("x")
    with pytest.raises(dt.InvalidArgumentError, match="not empty"):
        dt.Project.create(game, folder, streams=["gf2/level"])


def test_an_empty_project_gets_the_sources_of_its_workspace(sample, tmp_path):
    game, keys, root = sample
    original = snapshot(root)
    project = dt.Project.create(game, tmp_path / "doc", name="Yard Mod", empty=True)
    assert project.id == "yard-mod" and project.entries() == []
    assert (project.root / "project.toml").is_file()
    assert not (project.root / "workspace").exists() and not (project.root / "src").exists()
    with pytest.raises(dt.InvalidArgumentError, match="empty project"):
        dt.Project.create(game, tmp_path / "x", streams=["gf2/level"], empty=True)
    assert project.sources()["added"] == []

    # An edit in the project's own workspace gets a source file.
    ws = game.workspace(project.root / "workspace")
    ws.set_data(keys["vfx"], b"ember\0")
    report = project.sources()
    assert len(report["added"]) == 1 and report["notes"] == []
    entry = project.entries()[0]
    assert entry["keys"] == [keys["vfx"]] and entry["kind"] == "raw"
    assert (project.root / entry["files"][0]).read_bytes() == b"ember\0"
    assert project.status()["to_build"] == 0
    assert project.sources()["added"] == []
    # Nothing of it is in the game folder.
    assert snapshot(root) == original
    assert not (root / "_doctor_toolkit").exists()

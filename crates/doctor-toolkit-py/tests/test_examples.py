"""The scripts of `examples/python` on the synthetic game."""

import subprocess
import sys
from pathlib import Path

import doctor_toolkit as dt
from conftest import snapshot

EXAMPLES = Path(__file__).resolve().parents[3] / "examples" / "python"


def run(script, *args):
    result = subprocess.run(
        [sys.executable, str(EXAMPLES / script), *map(str, args)],
        capture_output=True,
        text=True,
        stdin=subprocess.DEVNULL,
        check=True,
    )
    return result.stdout, result.stderr


def test_export_textures(sample, tmp_path):
    _, _, root = sample
    out = tmp_path / "png"
    stdout, _ = run("export_textures.py", root, "gf2/level", out)
    assert "wall: 4x4 DXT1c" in stdout
    assert (out / "wall.png").read_bytes().startswith(b"\x89PNG")
    run("export_textures.py", root, "gf2/level", tmp_path / "dds", "--dds")
    assert (tmp_path / "dds" / "wall.dds").read_bytes().startswith(b"DDS ")


def test_simgroup_positions(sample):
    _, _, root = sample
    stdout, stderr = run("simgroup_positions.py", root, "entity")
    assert "don_vito  (1.00, 2.00, 3.00)" in stdout
    assert "2 object(s)" in stderr
    stdout, _ = run("simgroup_positions.py", root, "38523fc3", "--csv")
    assert stdout.splitlines()[0].startswith("stream,resource,name")
    assert len(stdout.splitlines()) == 3


def test_replace_strings(sample):
    game, keys, root = sample
    original = snapshot(root)
    stdout, _ = run("replace_strings.py", root, "o", "0", "--dry-run")
    assert "4 string(s) in 2 table(s)" in stdout
    assert not (root / "_doctor_toolkit").exists()
    run("replace_strings.py", root, "o", "0")
    assert snapshot(root) == original
    ws = game.workspace()
    assert ws.string_table(keys["strings"]).items() == [
        (1, "Hell0"),
        (2, "W0rld"),
        (3, "D0n C0rle0ne"),
    ]
    stdout, _ = run("replace_strings.py", root, "Y", "J", "--stream", "gf2/text/abcd", "--apply")
    assert "rebuilt gf2/level" in stdout and "rebuilt gf2/text/abcd" in stdout
    assert dt.Game(root).resource(keys["text"]).string_table()[1] == "Jes"

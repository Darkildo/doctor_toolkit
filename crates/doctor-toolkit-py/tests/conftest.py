import hashlib
import os
import shutil
from pathlib import Path

import pytest

import doctor_toolkit as dt


@pytest.fixture
def sample(tmp_path, monkeypatch):
    """A fresh synthetic game: `(game, keys, root)`. The index cache goes to a temp dir."""
    monkeypatch.setenv("DOCTOR_TOOLKIT_CACHE_DIR", str(tmp_path / "cache"))
    root = tmp_path / "game"
    root.mkdir()
    keys = dt.testing.build_sample_game(root)
    return dt.Game(root), keys, root


def snapshot(root: Path) -> dict:
    """SHA-256 of every file under `root` except the tool directory."""
    out = {}
    for path in sorted(root.rglob("*")):
        rel = path.relative_to(root).as_posix()
        if rel.startswith("_doctor_toolkit") or not path.is_file():
            continue
        out[rel] = hashlib.sha256(path.read_bytes()).hexdigest()
    return out


def corpus_dir():
    value = os.environ.get("EARS_GF2_DIR")
    if not value:
        return None
    path = Path(value)
    return path if path.is_dir() else None


@pytest.fixture(scope="session")
def corpus():
    """The installed game named by `EARS_GF2_DIR` (read only); skips without it."""
    path = corpus_dir()
    if path is None:
        pytest.skip("EARS_GF2_DIR is not set")
    return dt.Game(path)


# Files of the real game copied for write tests; the game itself is only read.
COPIED = [
    "gf2/frontend.str",
    "gf2/frontend.toc",
    "gf2/frontend_global.toc",
    "gf2/text/dd7d7050.str",
]


@pytest.fixture
def corpus_copy(tmp_path, monkeypatch):
    """A copy of a few real streams in a temp dir: `(game, root, original snapshot)`."""
    path = corpus_dir()
    if path is None:
        pytest.skip("EARS_GF2_DIR is not set")
    monkeypatch.setenv("DOCTOR_TOOLKIT_CACHE_DIR", str(tmp_path / "cache"))
    before = {rel: hashlib.sha256((path / rel).read_bytes()).hexdigest() for rel in COPIED}
    root = tmp_path / "game"
    for rel in COPIED:
        (root / rel).parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(path / rel, root / rel)
    yield dt.Game(root), root
    after = {rel: hashlib.sha256((path / rel).read_bytes()).hexdigest() for rel in COPIED}
    assert after == before, "the real game files changed"

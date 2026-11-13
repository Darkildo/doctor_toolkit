"""`doctor-toolkit edit … --format glb` with headless Blender as the editor
(`link_edit.py`): the CLI exports the model, Blender opens it through the
add-on and saves twice, the CLI imports the saves into the workspace as one
operation. Run by `run-tests.sh --cli PATH` on the copied game:

    python3 link_e2e.py --cli doctor-toolkit --blender blender --game COPY \\
        --module-path DIR --stream STREAM --model NAME --work DIR
"""

from __future__ import annotations

import argparse
import os
import shlex
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)

import glb  # noqa: E402

MOVED = 2 * 0.25  # link_edit.py moves vertex 0 twice


def main() -> int:
    p = argparse.ArgumentParser()
    p.add_argument("--cli", required=True)
    p.add_argument("--blender", required=True)
    p.add_argument("--game", required=True)
    p.add_argument("--module-path", required=True)
    p.add_argument("--stream", required=True)
    p.add_argument("--model", default="")
    p.add_argument("--work", required=True)
    a = p.parse_args()
    sys.path.insert(0, a.module_path)
    import doctor_toolkit as dt

    game = dt.Game(a.game)
    meshes = [r for r in game.resources("mesh", streams=[a.stream])]
    if a.model:
        meshes = [r for r in meshes if r.model().name.lower() == a.model.lower()]
    key = meshes[0].key
    ws_dir = os.path.join(a.work, "ws-link")
    before = glb.triangles(game.workspace(ws_dir).export_model(key))

    editor = " ".join(
        shlex.quote(w)
        for w in [
            a.blender, "-b", "--factory-startup", "--python",
            os.path.join(HERE, "link_edit.py"), "--", "{file}",
            "--module-path", a.module_path,
        ]
    )
    command = [
        a.cli, "edit", a.game, key, "--format", "glb", "--with", editor, "--wait",
        "--file-dir", os.path.join(a.work, "link"), "--workspace-dir", ws_dir,
    ]
    print("running:", " ".join(shlex.quote(c) for c in command), flush=True)
    run = subprocess.run(command, stdin=subprocess.DEVNULL, capture_output=True, text=True)
    output = run.stdout + run.stderr
    print("\n".join(l for l in output.splitlines() if "INFO" not in l))
    failures = []
    if run.returncode != 0:
        failures.append(f"edit exited with {run.returncode}")
    if "imported: Import model" not in output:
        failures.append("no save was imported")
    ws = game.workspace(ws_dir)
    labels = [op["label"] for op in ws.history]
    print("workspace:", labels)
    if len(labels) != 1:
        failures.append(f"{len(labels)} operations instead of one")
    after = glb.triangles(ws.export_model(key))
    pairs, removed, added = glb.match(before, after)
    rounded = lambda tris: {tuple(round(c, 5) for c in p) for t in tris for p in t[0]}  # noqa: E731
    gone = rounded(removed) - rounded(added)
    come = rounded(added) - rounded(removed)
    moved = any(
        abs(q[0] - p[0] - MOVED) < 1e-3 and abs(q[1] - p[1]) < 1e-3 and abs(q[2] - p[2]) < 1e-3
        for p in gone
        for q in come
    )
    print(f"{len(pairs)} triangles equal, {len(removed)} removed, {len(added)} added")
    if not removed or len(removed) != len(added) or not moved:
        failures.append("the moved vertex is not in the workspace")
    if failures:
        print("FAILED:", *failures, sep="\n  ")
        return 1
    print("link with Blender OK")
    return 0


if __name__ == "__main__":
    sys.exit(main())

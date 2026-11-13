"""Round trip of a game model through Blender, run in headless Blender:

    blender -b --factory-startup --python roundtrip.py -- \\
        --game "/path/to/The Godfather II" --work /tmp/work [--module-path DIR]

1. The model's stream and the tables of contents are copied into `--work`;
   the game directory is only read.
2. The add-on is registered, the model is imported through it (the
   workspace version as GLB, Blender's glTF importer).
3. Exported back without changes: the geometry of the workspace equals
   the original (positions exactly, UVs within 1e-3).
4. One vertex is moved: exactly the triangles that use it change.

`tools/blender/run-tests.sh` runs this with a Blender configuration in a
temporary folder. Exit code 0 when every check passes.
"""

from __future__ import annotations

import argparse
import os
import shutil
import sys
import traceback

import bpy

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
sys.path.insert(0, os.path.dirname(HERE))

import glb  # noqa: E402

DEFAULT_STREAM = "mp_rooftops/mp_rooftops"
MOVE = (0.25, 0.0, 0.0)


def args():
    argv = sys.argv[sys.argv.index("--") + 1 :] if "--" in sys.argv else []
    p = argparse.ArgumentParser()
    p.add_argument("--game", required=True)
    p.add_argument("--work", required=True)
    p.add_argument("--module-path", default="")
    p.add_argument("--stream", default=DEFAULT_STREAM)
    p.add_argument("--model", default="", help="mesh name; default: the largest mesh")
    p.add_argument("--extra", action="append", default=[], help="another stream to copy")
    return p.parse_args(argv)


def copy_game(game: str, work: str, streams: list[str], dt) -> str:
    """Copies the streams and every table of contents."""
    real = dt.Game(game)
    root = os.path.join(work, "game")
    if os.path.exists(root):
        shutil.rmtree(root)
    paths = {f["name"]: f["path"] for f in real.stream_files()}
    rels = [paths[s] for s in streams] + list(real.tocs)
    for rel in rels:
        source = os.path.join(game, rel)
        if not os.path.exists(source):
            continue
        target = os.path.join(root, rel)
        os.makedirs(os.path.dirname(target), exist_ok=True)
        shutil.copyfile(source, target)
    return root


def pick_mesh(game, stream: str, name: str):
    best = None
    for res in game.resources("mesh", streams=[stream]):
        model = res.model()
        if name and model.name.lower() != name.lower():
            continue
        size = sum(r["index_count"] for r in model.records)
        if best is None or size > best[0]:
            best = (size, res, model.name)
    if best is None:
        raise SystemExit(f"no mesh {name!r} in {stream}")
    return best[1], best[2]


def clear_scene():
    for obj in list(bpy.data.objects):
        bpy.data.objects.remove(obj, do_unlink=True)
    for mesh in list(bpy.data.meshes):
        bpy.data.meshes.remove(mesh)
    for arm in list(bpy.data.armatures):
        bpy.data.armatures.remove(arm)


def compare(before: list, after: list, moved_from=None, moved_to=None) -> list[str]:
    """Problems found comparing two triangle lists; empty when they match
    (positions within 1e-4, UVs within 1e-3). With a moved vertex, exactly
    the triangles that use it may differ."""
    problems = []
    matched, removed, added = glb.match(before, after, tolerance=1e-4)
    if len(before) != len(after):
        problems.append(f"{len(before)} triangles before, {len(after)} after")
    if moved_from is None:
        if removed or added:
            problems.append(f"{len(removed)} triangle(s) lost, {len(added)} new")
    else:
        near = lambda p, q: max(abs(a - b) for a, b in zip(p, q)) < 1e-4  # noqa: E731
        if not removed:
            problems.append("the moved vertex changed no triangle")
        bad_removed = [t for t in removed if not any(near(p, moved_from) for p in t[0])]
        bad_added = [t for t in added if not any(near(p, moved_to) for p in t[0])]
        if bad_removed or bad_added or len(removed) != len(added):
            problems.append(
                f"other triangles changed: {len(bad_removed)} lost, {len(bad_added)} new "
                f"({len(removed)} removed, {len(added)} added)"
            )
    worst_pos, worst_uv, worst_weight, worst_color = glb.deviation(matched)
    print(
        f"  {len(matched)} triangles equal (positions within {worst_pos:.2e}, UVs within "
        f"{worst_uv:.2e}, bone weights within {worst_weight:.2e}, colours within "
        f"{worst_color:.2e}), {len(removed)} removed, {len(added)} added"
    )
    if worst_color > 2.5 / 255:
        problems.append(f"vertex colours differ by up to {worst_color:.4f}")
    if worst_uv > 1e-3:
        problems.append(f"UVs differ by up to {worst_uv:.5f}")
    if worst_weight > 2e-3:
        problems.append(f"bone weights differ by up to {worst_weight:.5f}")
    return problems


def main() -> int:
    a = args()
    addon_root = os.path.dirname(HERE)
    if addon_root not in sys.path:
        sys.path.insert(0, addon_root)
    import doctor_toolkit_blender as addon
    from doctor_toolkit_blender import core

    dt = core.load_toolkit(a.module_path)
    addon.register()
    print("add-on registered; doctor_toolkit", getattr(dt, "__version__", "?"))
    os.makedirs(a.work, exist_ok=True)
    os.environ.setdefault("DOCTOR_TOOLKIT_CACHE_DIR", os.path.join(a.work, "cache"))
    root = copy_game(a.game, a.work, [a.stream, *a.extra], dt)
    game = dt.Game(root)
    res, name = pick_mesh(game, a.stream, a.model)
    key = res.key
    print(f"model {name} ({key})")
    failures = []

    def original_triangles(ws):
        return glb.triangles(ws.export_model(key, core.game_index(dt, root)))

    def step(label: str, moved: bool) -> None:
        clear_scene()
        ws_dir = os.path.join(a.work, f"ws-{label}")
        shutil.rmtree(ws_dir, ignore_errors=True)
        ws = game.workspace(ws_dir)
        before = original_triangles(ws)
        objects = core.import_model(dt, root, key, ws_dir)
        meshes = [o for o in objects if o.type == "MESH"]
        print(f"[{label}] imported {len(objects)} object(s), {len(meshes)} mesh(es)")
        moved_from = moved_to = None
        if moved:
            obj = meshes[0]
            vertex = obj.data.vertices[0]
            # Blender is +Z up; the game and glTF +Y up: (x, y, z) -> (x, -z, y).
            gx, gy, gz = vertex.co.x, vertex.co.z, -vertex.co.y
            moved_from = (gx, gy, gz)
            vertex.co.x += MOVE[0]
            moved_to = (gx + MOVE[0], gy, gz)
            print(f"[{label}] moved vertex 0 of {obj.name} from {moved_from} to {moved_to}")
        check = core.prepare_for_export(objects)
        for line in check.fixed + check.warnings + check.errors:
            print(f"[{label}] check: {line}")
        if check.errors:
            failures.append(f"{label}: {check.errors}")
            return
        keep = os.path.join(a.work, f"{label}.glb")
        plan = core.export_to_workspace(
            dt, root, key, objects, dry_run=True, workspace_dir=ws_dir, glb_path=keep
        )
        for line in core.plan_lines(plan):
            print(f"[{label}] plan: {line}")
        summary = core.export_to_workspace(dt, root, key, objects, workspace_dir=ws_dir)
        ws = game.workspace(ws_dir)
        print(f"[{label}] recorded: {[op['label'] for op in ws.history]}")
        if summary.get("meshes") != [key]:
            failures.append(f"{label}: replaced {summary.get('meshes')}")
        after = original_triangles(ws)
        problems = compare(before, after, moved_from, moved_to)
        for p in problems:
            failures.append(f"{label}: {p}")
        print(f"[{label}] {len(before)} triangles before, {len(after)} after: "
              + ("OK" if not problems else "; ".join(problems)))

    try:
        step("unchanged", moved=False)
        step("moved", moved=True)
    except Exception:  # noqa: BLE001
        traceback.print_exc()
        failures.append("exception")
    addon.unregister()
    if failures:
        print("FAILED:", *failures, sep="\n  ")
        return 1
    print("round trip OK")
    return 0


if __name__ == "__main__":
    code = main()
    sys.stdout.flush()
    os._exit(code)

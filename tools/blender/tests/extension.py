"""Checks the installed extension in headless Blender: the add-on is
enabled, its bundled `doctor_toolkit` module imports, and the operators of
the panel work on a copy of a game:

    blender -b --python extension.py -- --game COPY --query NAME

open the game, search, import the first model found, check the export
(plan), export into the workspace. `run-tests.sh --extension ZIP` installs
the extension into a temporary Blender user folder and runs this.
"""

from __future__ import annotations

import argparse
import os
import sys

import addon_utils
import bpy


def main() -> int:
    argv = sys.argv[sys.argv.index("--") + 1 :] if "--" in sys.argv else []
    p = argparse.ArgumentParser()
    p.add_argument("--game", required=True)
    p.add_argument("--query", required=True)
    a = p.parse_args(argv)

    enabled = [m for m in addon_utils.modules() if m.__name__.endswith("doctor_toolkit_blender")]
    if not enabled or not addon_utils.check(enabled[0].__name__)[1]:
        print("FAILED: the extension is not enabled")
        return 1
    import doctor_toolkit

    print("module:", doctor_toolkit.__file__)
    if "extensions" not in doctor_toolkit.__file__:
        print("FAILED: the module does not come from the extension's wheel")
        return 1

    for obj in list(bpy.data.objects):
        bpy.data.objects.remove(obj, do_unlink=True)
    state = bpy.context.scene.doctor_toolkit
    state.game_dir = a.game
    steps = [
        ("open game", lambda: bpy.ops.doctor_toolkit.open_game()),
        ("search", lambda: setattr(state, "query", a.query) or bpy.ops.doctor_toolkit.search()),
        ("import", lambda: bpy.ops.doctor_toolkit.import_model()),
        ("check", lambda: bpy.ops.doctor_toolkit.check_export()),
        ("export", lambda: bpy.ops.doctor_toolkit.export_model()),
    ]
    for name, step in steps:
        result = step()
        print(f"{name}: {result} — {state.message}")
        if "FINISHED" not in result:
            print(f"FAILED at {name}")
            return 1
        if name == "search" and not state.results:
            print("FAILED: nothing found")
            return 1
    for line in state.plan:
        print("plan:", line.text)
    ws = os.path.join(a.game, "_doctor_toolkit", "workspace")
    history = doctor_toolkit.Game(a.game).workspace(ws).history
    print("workspace:", [op["label"] for op in history])
    if not history:
        print("FAILED: nothing recorded")
        return 1
    print("extension OK")
    return 0


if __name__ == "__main__":
    code = main()
    sys.stdout.flush()
    os._exit(code)

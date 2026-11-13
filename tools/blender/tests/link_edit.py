"""A scripted Blender session as the "external editor" of a model:

    blender -b --factory-startup --python link_edit.py -- FILE --module-path DIR

Opens the linked GLB through the add-on, moves vertex 0 by 0.25 m along X
and saves (the toolkit imports the save), then moves it once more and saves
again a second later: the two saves make one workspace operation.
"""

from __future__ import annotations

import argparse
import os
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.dirname(HERE))

import bpy  # noqa: E402

STEP = 0.25


def main() -> int:
    argv = sys.argv[sys.argv.index("--") + 1 :]
    p = argparse.ArgumentParser()
    p.add_argument("file")
    p.add_argument("--module-path", default="")
    a = p.parse_args(argv)
    import doctor_toolkit_blender as addon
    from doctor_toolkit_blender import core

    core.load_toolkit(a.module_path)
    addon.register()
    for obj in list(bpy.data.objects):
        bpy.data.objects.remove(obj, do_unlink=True)
    if "FINISHED" not in bpy.ops.doctor_toolkit.link_open(filepath=a.file):
        return 1
    mesh = next(o for o in bpy.context.scene.objects if o.type == "MESH")
    for save in range(2):
        mesh.data.vertices[0].co.x += STEP
        if "FINISHED" not in bpy.ops.doctor_toolkit.link_save():
            return 1
        print(f"link_edit: save {save + 1} written", flush=True)
        time.sleep(1.5)
    addon.unregister()
    return 0


if __name__ == "__main__":
    code = main()
    sys.stdout.flush()
    os._exit(code)

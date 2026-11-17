#!/usr/bin/env python3
"""Helpers of research/bench_platform.sh (stage 80).

Usage:
    bench_platform.py guids EXPORT_DIR > guids.tsv
        GUID, class and stream of every DoctorCounter and DoctorCEcho object
        in the SimGroups written by `doctor-toolkit logic-export --type
        simgroup` (with DOCTOR_TOOLKIT_CLASSES naming the class file).
    bench_platform.py world WORLD.json guids.tsv
        WORLD.json is `doctor-toolkit live objects --all --json`: says for
        every object of guids.tsv whether the game has it and whether its
        listener is switched on.
"""
import glob
import json
import os
import sys

CLASSES = ("DoctorCounter", "DoctorCEcho")


def guids(root):
    for path in sorted(glob.glob(root + "/**/*simgroup*.json", recursive=True)):
        stream = os.path.relpath(os.path.dirname(path), root)
        for o in json.load(open(path))["objects"]:
            if o.get("class_name") in CLASSES:
                print(o["guid"], o["class_name"], stream)


def plain(guid):
    return "".join(c for c in guid.lower() if c in "0123456789abcdef")


def world(path, table):
    objects = json.load(open(path))["objects"]
    alive = {plain(o["guid"]): o for o in objects}
    print("objects in the game:", len(alive))
    for line in open(table):
        guid, cls, stream = line.split()
        o = alive.get(plain(guid))
        if o is None:
            print("  %s %s of %s: not in the game" % (cls, guid, stream))
        else:
            print("  %s %s of %s: in the game, class known to the live module: %s, listening: %s"
                  % (cls, guid, stream, o.get("class"), o.get("listening")))


if __name__ == "__main__":
    if len(sys.argv) == 3 and sys.argv[1] == "guids":
        guids(sys.argv[2])
    elif len(sys.argv) == 4 and sys.argv[1] == "world":
        world(sys.argv[2], sys.argv[3])
    else:
        sys.exit(__doc__)

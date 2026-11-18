#!/usr/bin/env python3
"""Stage 107 on the bench: a base mod and a project that change one zone.

    setup.py GAME_COPY BASE_WORKSPACE PROJECT_DIR

In `gf2/cuba/local/yuri/high/terrain3` (the zone of the world grid at the
spawn point of Havana):
  * the base mod (BASE_WORKSPACE): every second colour texture tinted
    magenta, and the first of them stored at half resolution (another size
    of header and data: seen in the table of loaded resources);
  * the project (PROJECT_DIR/workspace): the other colour textures tinted
    green, the first of them at double resolution, and one texture of the
    base mod tinted yellow on top of it (the project wins as a patch).

Nothing is written into GAME_COPY. Prints what it did as JSON lines.
"""
import json
import os
import re
import subprocess
import sys
import tempfile

from PIL import Image

TOOL = os.environ.get("T", "doctor-toolkit")
STREAM = "gf2/cuba/local/yuri/high/terrain3"


def run(*args):
    result = subprocess.run([TOOL, *args], capture_output=True, text=True)
    if result.returncode != 0:
        sys.exit(f"{' '.join(args)}\n{result.stdout}{result.stderr}")
    return result.stdout


def textures(game, out):
    run("textures", "--png", "-o", out, os.path.join(game, STREAM + ".str"))
    found = []
    for root, _, files in os.walk(out):
        for name in sorted(files):
            if not name.endswith(".png"):
                continue
            path = os.path.join(root, name)
            with Image.open(path) as im:
                w, h = im.size
            stem = name[:-4]
            if w == h and 64 <= w <= 256 and not re.search(r"_[0-9a-f]{8}$", stem):
                found.append((stem, path, w))
    colour = [t for t in found if t[0].endswith(("_c", "_d", "_ca"))]
    return colour if len(colour) >= 6 else found


def tint(path, out, colour, size=None):
    with Image.open(path) as im:
        im = im.convert("RGBA")
        if size:
            im = im.resize((size, size))
        a = im.split()[3]
        mixed = Image.blend(im, Image.new("RGBA", im.size, colour + (255,)), 0.6)
        mixed.putalpha(a)
        mixed.save(out)


def main():
    game, base, project = sys.argv[1:4]
    with tempfile.TemporaryDirectory() as tmp:
        pngs = textures(game, os.path.join(tmp, "png"))
        out = os.path.join(tmp, "t.png")
        # The flowers of the agave by the gates are in nobody else's zone and
        # in view from the spawn point: they go to the project; the base mod
        # keeps the posters.
        agave = [t for t in pngs if t[0].startswith("agave")]
        rest = [t for t in pngs if not t[0].startswith("agave")]
        mine = rest[0::2]
        theirs = agave + rest[1::2]
        for i, (name, path, size) in enumerate(mine):
            tint(path, out, (255, 0, 255), size // 2 if i == 0 else None)
            run("texture-import", game, STREAM, name, out, "--workspace-dir", base)
            print(json.dumps({"mod": "base", "texture": name, "size": size // 2 if i == 0 else size}))
        ws = os.path.join(project, "workspace")
        for i, (name, path, size) in enumerate(theirs):
            tint(path, out, (0, 255, 0), size // 2 if i == 0 else None)
            run("texture-import", game, STREAM, name, out, "--workspace-dir", ws)
            print(json.dumps({"mod": "project", "texture": name, "size": size // 2 if i == 0 else size}))
        name, path, size = mine[0]
        tint(path, out, (255, 255, 0))
        run("texture-import", game, STREAM, name, out, "--workspace-dir", ws)
        print(json.dumps({"mod": "project", "texture": name, "on": "base", "size": size}))

    # Scripts of the root stream of every level: the game keeps their data in
    # memory as they are in the stream, so they are compared there byte by
    # byte. The base mod changes a letter of one, the project makes another
    # longer: one stream, changed by both.
    root = "gf2/globals_global"
    rows = []
    for line in run("list", os.path.join(game, root + ".str")).splitlines()[1:]:
        m = re.match(r"\s*(\d+)\s+(\S+)\s+\S+\s+\S+\s+((?:[0-9a-f]{8} ?)+)\s+(\d+)", line)
        if m and m[2] == "lua":
            rows.append((int(m[1]), int(m[4])))
    small = sorted(rows, key=lambda r: r[1])
    with tempfile.TemporaryDirectory() as tmp:
        def data_of(index):
            out = os.path.join(tmp, f"x{index}")
            run("extract", os.path.join(game, root + ".str"), "--index", str(index), "-o", out)
            (name,) = os.listdir(out)
            with open(os.path.join(out, name), "rb") as f:
                return bytearray(f.read())

        def replace(index, data, ws, who):
            path = os.path.join(tmp, f"new{index}.bin")
            with open(path, "wb") as f:
                f.write(data)
            run("replace", game, root, "--index", str(index), "--data", path, "--workspace-dir", ws)
            print(json.dumps({"mod": who, "stream": root, "lua": index, "size": len(data)}))

        data = data_of(small[0][0])
        at = data.find(b".lua\0")
        if at <= 0:
            sys.exit("no source name in the script")
        data[at - 1] = ord("X") if data[at - 1] != ord("X") else ord("Y")
        replace(small[0][0], data, base, "base")
        replace(small[1][0], data_of(small[1][0]) + bytes(37), os.path.join(project, "workspace"), "project")


main()

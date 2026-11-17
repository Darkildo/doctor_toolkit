#!/usr/bin/env python3
"""Stage 100: the test workspace of the bench — every kind of change the
module has to put into streams in memory, in three cities and in the root
streams.

    mkws.py GAME_COPY WORKSPACE [CITY...]     (cities: cuba miami queens)

For every city, in the always loaded `gf2/CITY/local`:
  * three colour textures tinted (replaced, the same size);
  * one texture at half and one at double resolution (replaced, another size:
    header and data);
  * a texture cloned under new ids into the same stream (added resources);
and a new stream `gf2/doctor/s100CITY` (parent `gf2/CITY/local`) with a clone
of a texture; in `gf2/CITY_global` one texture tinted and one cloned.
In `gf2/globals_global` (the root of every level): one texture tinted, one at
half resolution, one cloned; and scripts (`lua`), whose data the game keeps
in memory as they are in the stream, so that they can be compared there
byte by byte: one with a letter of its source name changed (replaced, the
same size), one with 37 bytes after the chunk (replaced, larger), one cloned
into the stream and one cloned into every new stream (added).
With cuba: the colour textures of the zone at the spawn point,
`gf2/cuba/local/yuri/high/terrain3`, tinted (a stream of the world grid).

Nothing is written into GAME_COPY: every change is recorded in WORKSPACE.
Prints what it did as JSON lines (`mkws.jsonl` in the workspace folder).
"""
import json
import os
import re
import subprocess
import sys
import tempfile

from PIL import Image

TOOL = os.environ.get("T", "doctor-toolkit")


def run(*args):
    result = subprocess.run([TOOL, *args], capture_output=True, text=True)
    if result.returncode != 0:
        sys.exit(f"{' '.join(args)}\n{result.stdout}{result.stderr}")
    return result.stdout


def resources(game, stream):
    """Rows of `list`: index, type, ids, size."""
    rows = []
    for line in run("list", os.path.join(game, stream + ".str")).splitlines()[1:]:
        m = re.match(r"\s*(\d+)\s+(\S+)\s+\S+\s+\S+\s+((?:[0-9a-f]{8} ?)+)\s+(\d+)", line)
        if m:
            rows.append((int(m[1]), m[2], m[3].split(), int(m[4])))
    return rows


def textures(game, stream, out):
    """PNG files of the colour textures of a stream that are safe to pick:
    square, 64 to 256 pixels, a name used once."""
    run("textures", "--png", "-o", out, os.path.join(game, stream + ".str"))
    found = []
    for root, _, files in os.walk(out):
        for name in sorted(files):
            if not name.endswith(".png"):
                continue
            path = os.path.join(root, name)
            with Image.open(path) as im:
                w, h = im.size
            stem = name[:-4]
            # Clones carry `_HASH`; keep plain names of colour maps.
            if w == h and 64 <= w <= 256 and not re.search(r"_[0-9a-f]{8}$", stem):
                found.append((stem, path, w))
    colour = [t for t in found if t[0].endswith(("_c", "_d", "_ca"))]
    return colour if len(colour) >= 6 else found


def tint(path, out, colour):
    with Image.open(path) as im:
        im = im.convert("RGBA")
        r, g, b, a = im.split()
        solid = Image.new("RGBA", im.size, colour + (255,))
        mixed = Image.blend(im, solid, 0.6)
        mixed.putalpha(a)
        mixed.save(out)


def scale(path, out, size):
    with Image.open(path) as im:
        im.convert("RGBA").resize((size, size)).save(out)


def main():
    game, ws = sys.argv[1], sys.argv[2]
    cities = sys.argv[3:] or ["cuba", "miami", "queens"]
    log = []
    wsargs = ["--workspace-dir", ws]

    def note(**kw):
        log.append(kw)
        print(json.dumps(kw))

    def change_textures(stream, tints, half, double, clones, new_stream=None, parent=None):
        with tempfile.TemporaryDirectory() as tmp:
            pngs = textures(game, stream, os.path.join(tmp, "png"))
            if len(pngs) < tints + half + double + 1:
                sys.exit(f"{stream}: only {len(pngs)} textures to pick from")
            picked = iter(pngs)
            for _ in range(tints):
                name, path, _ = next(picked)
                out = os.path.join(tmp, "t.png")
                tint(path, out, (255, 0, 255))
                run("texture-import", game, stream, name, out, *wsargs)
                note(stream=stream, change="replaced, same size", texture=name)
            for _ in range(half):
                name, path, size = next(picked)
                out = os.path.join(tmp, "h.png")
                scale(path, out, size // 2)
                run("texture-import", game, stream, name, out, *wsargs)
                note(stream=stream, change="replaced, smaller", texture=name, size=size // 2)
            for _ in range(double):
                name, path, size = next(picked)
                out = os.path.join(tmp, "d.png")
                scale(path, out, size * 2)
                run("texture-import", game, stream, name, out, *wsargs)
                note(stream=stream, change="replaced, larger", texture=name, size=size * 2)
        rows = resources(game, stream)
        headers = [r for r in rows if r[1] == "tg4h"]
        # The last textures of the stream: not the ones replaced above.
        for n in range(clones):
            index = headers[-1 - n][0]
            run("clone-resource", game, stream, "--index", str(index),
                "--seed", f"s100 {stream} {n}", *wsargs)
            note(stream=stream, change="added (clone in the stream)", index=index,
                 ids=headers[-1 - n][2])
        if new_stream:
            run("add-stream", game, new_stream, "--parent", parent, *wsargs)
            index = headers[len(headers) // 2][0]
            run("clone-resource", game, stream, "--index", str(index), "--to", new_stream,
                "--seed", f"s100 {new_stream}", *wsargs)
            note(stream=new_stream, change="new stream with a cloned texture",
                 source=stream, index=index, parent=parent)

    for city in cities:
        local = f"gf2/{city}/local"
        change_textures(local, 3, 1, 1, 1, new_stream=f"gf2/doctor/s100{city}", parent=local)
        change_textures(f"gf2/{city}_global", 1, 0, 0, 1)
    if "cuba" in cities:
        change_textures("gf2/cuba/local/yuri/high/terrain3", 12, 0, 0, 0)
    root = "gf2/globals_global"
    change_textures(root, 1, 1, 0, 1)

    # Scripts: kept in memory as they are in the stream.
    luas = [r for r in resources(game, root) if r[1] == "lua"]
    small = sorted(luas, key=lambda r: r[3])
    with tempfile.TemporaryDirectory() as tmp:
        def data_of(index):
            out = os.path.join(tmp, f"x{index}")
            run("extract", os.path.join(game, root + ".str"), "--index", str(index), "-o", out)
            (name,) = os.listdir(out)
            with open(os.path.join(out, name), "rb") as f:
                return bytearray(f.read())

        def replace(index, data, change):
            path = os.path.join(tmp, f"new{index}.bin")
            with open(path, "wb") as f:
                f.write(data)
            run("replace", game, root, "--index", str(index), "--data", path, *wsargs)
            note(stream=root, change=change, index=index, size=len(data))

        same, larger, cloned, sent = small[0], small[1], small[2], small[3]
        data = data_of(same[0])
        at = data.find(b".lua\0")
        if at <= 0:
            sys.exit("no source name in the script")
        data[at - 1] = ord("X") if data[at - 1] != ord("X") else ord("Y")
        replace(same[0], data, "replaced, same size (script)")
        replace(larger[0], data_of(larger[0]) + bytes(37), "replaced, larger (script)")
        run("clone-resource", game, root, "--index", str(cloned[0]),
            "--seed", "s100 lua", *wsargs)
        note(stream=root, change="added (clone of a script in the stream)", index=cloned[0])
        for city in cities:
            new = f"gf2/doctor/s100{city}"
            run("clone-resource", game, root, "--index", str(sent[0]), "--to", new,
                "--seed", f"s100 lua {city}", *wsargs)
            note(stream=new, change="added (clone of a script in the new stream)",
                 source=root, index=sent[0])
    with open(os.path.join(ws, "mkws.jsonl"), "w") as f:
        for entry in log:
            f.write(json.dumps(entry) + "\n")


if __name__ == "__main__":
    main()

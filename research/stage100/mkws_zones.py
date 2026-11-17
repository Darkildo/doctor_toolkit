#!/usr/bin/env python3
"""Stage 100: the workspace of the timing runs — a change in every zone of
a city, so that every load of a zone by the world grid goes through the
module (a composed stream, or a rebuilt one with stage 46).

    mkws_zones.py GAME_COPY WORKSPACE [CITY] [DETAIL]   (default: cuba high)

In every stream `gf2/CITY/local/*/DETAIL/*.str` that has textures: the data
of its last texture are replaced by data of the same size (every byte
changed), and its first texture is cloned under new ids into the stream
(two added resources). Nothing is written into GAME_COPY.
"""
import os
import re
import subprocess
import sys
import tempfile

TOOL = os.environ.get("T", "doctor-toolkit")


def run(*args):
    result = subprocess.run([TOOL, *args], capture_output=True, text=True)
    if result.returncode != 0:
        sys.exit(f"{' '.join(args)}\n{result.stdout}{result.stderr}")
    return result.stdout


def resources(path):
    rows = []
    for line in run("list", path).splitlines()[1:]:
        m = re.match(r"\s*(\d+)\s+(\S+)\s+\S+\s+\S+\s+((?:[0-9a-f]{8} ?)+)\s+(\d+)", line)
        if m:
            rows.append((int(m[1]), m[2], int(m[4])))
    return rows


def main():
    game, ws = sys.argv[1], sys.argv[2]
    city = sys.argv[3] if len(sys.argv) > 3 else "cuba"
    detail = sys.argv[4] if len(sys.argv) > 4 else "high"
    root = os.path.join(game, "gf2", city, "local")
    streams = []
    for folder, _, files in os.walk(root):
        if os.path.basename(folder) != detail:
            continue
        for name in sorted(files):
            if name.endswith(".str"):
                rel = os.path.relpath(os.path.join(folder, name), game)
                streams.append(rel[:-4].replace(os.sep, "/"))
    streams.sort()
    changed = 0
    total = 0
    with tempfile.TemporaryDirectory() as tmp:
        for stream in streams:
            path = os.path.join(game, stream + ".str")
            rows = resources(path)
            headers = [r for r in rows if r[1] == "tg4h"]
            datas = [r for r in rows if r[1] == "tg4d"]
            if not headers or not datas:
                continue
            index = datas[-1][0]
            out = os.path.join(tmp, "x")
            run("extract", path, "--index", str(index), "-o", out)
            (name,) = os.listdir(out)
            with open(os.path.join(out, name), "rb") as f:
                data = bytes(b ^ 0x08 for b in f.read())
            os.remove(os.path.join(out, name))
            new = os.path.join(tmp, "new.bin")
            with open(new, "wb") as f:
                f.write(data)
            run("replace", game, stream, "--index", str(index), "--data", new,
                "--workspace-dir", ws)
            run("clone-resource", game, stream, "--index", str(headers[0][0]),
                "--seed", f"s100 zones {stream}", "--workspace-dir", ws)
            changed += 1
            total += os.path.getsize(path)
    print(f"{changed} of {len(streams)} streams of gf2/{city}/local/*/{detail} changed, "
          f"{total} bytes of stream files")


if __name__ == "__main__":
    main()

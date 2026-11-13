#!/usr/bin/env python3
"""Export every texture of one stream as PNG (or DDS).

    python export_textures.py "/path/to/The Godfather II" gf2/frontend out/
    python export_textures.py GAME gf2/frontend out/ --dds

Cube maps and volumes are written one PNG per layer. The game is only read.
"""

import argparse
import re
import sys
from pathlib import Path

import doctor_toolkit as dt


def safe(name: str) -> str:
    return re.sub(r"[^A-Za-z0-9_.-]", "_", name) or "unnamed"


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("game", help="game directory")
    parser.add_argument("stream", help="stream name, e.g. gf2/frontend")
    parser.add_argument("out", type=Path, help="output directory")
    parser.add_argument("--dds", action="store_true", help="write DDS files (lossless)")
    args = parser.parse_args()

    game = dt.Game(args.game)
    try:
        stream = game.stream(args.stream)
    except KeyError as error:
        print(error, file=sys.stderr)
        return 1
    args.out.mkdir(parents=True, exist_ok=True)
    written = 0
    for texture in stream.textures():
        base = args.out / safe(texture.name)
        if args.dds:
            base.with_suffix(".dds").write_bytes(texture.to_dds())
            written += 1
            continue
        for layer in range(texture.layer_count):
            suffix = f"_{layer}" if texture.layer_count > 1 else ""
            path = base.with_name(base.name + suffix + ".png")
            path.write_bytes(texture.to_png(layer))
            written += 1
        print(f"{texture.name}: {texture.width}x{texture.height} {texture.format}")
    print(f"wrote {written} file(s) to {args.out}")
    return 0


if __name__ == "__main__":
    sys.exit(main())

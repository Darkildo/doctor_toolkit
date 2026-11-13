#!/usr/bin/env python3
"""List the SimGroup objects of one class with their positions.

    python simgroup_positions.py "/path/to/The Godfather II" Entity
    python simgroup_positions.py GAME Entity --stream mp_mall --csv > entities.csv

The class is a name (`Entity`, matched without case) or a hexadecimal class
hash (`38523fc3`). `--stream` keeps streams whose name contains the text.
The game is only read.
"""

import argparse
import csv
import sys

import doctor_toolkit as dt


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("game", help="game directory")
    parser.add_argument("cls", metavar="class", help="class name or hash")
    parser.add_argument("--stream", help="only streams whose name contains this text")
    parser.add_argument("--csv", action="store_true", help="write CSV to stdout")
    args = parser.parse_args()

    wanted = args.cls.lower()
    game = dt.Game(args.game)
    streams = [s for s in game.streams if args.stream is None or args.stream in s]
    writer = csv.writer(sys.stdout) if args.csv else None
    if writer:
        writer.writerow(["stream", "resource", "name", "guid", "x", "y", "z", "model"])
    found = 0
    for resource in game.resources("simgroup", streams=streams):
        group = resource.parse()
        for obj in group["objects"]:
            names = {obj["class"], (obj.get("class_name") or "").lower()}
            if wanted not in names:
                continue
            x, y, z = obj.get("position") or (None, None, None)
            row = [resource.stream, resource.key, obj.get("name", ""), obj["guid"], x, y, z,
                   obj.get("model", "")]
            if writer:
                writer.writerow(row)
            else:
                where = f"({x:.2f}, {y:.2f}, {z:.2f})" if x is not None else "(no transform)"
                print(f"{resource.stream}  {obj.get('name') or obj['guid']}  {where}"
                      + (f"  model {obj['model']}" if "model" in obj else ""))
            found += 1
    print(f"{found} object(s)", file=sys.stderr)
    return 0


if __name__ == "__main__":
    sys.exit(main())

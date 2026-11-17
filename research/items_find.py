#!/usr/bin/env python3
"""Find template objects for the stage 75 experiments and the trader example.

Input: a directory made by
    doctor-toolkit logic-export "$EARS_GF2_DIR" -o DIR --type simgroup

Usage: items_find.py DIR CLASS [TEXT] [--stream PART] [--max N]

Prints stream, SimGroup resource index, object index, GUID and the attributes
of the objects of CLASS whose attribute values contain TEXT.
"""
import glob
import json
import os
import sys


def main():
    args = [a for a in sys.argv[1:] if not a.startswith("--")]
    root, cls = args[0], args[1]
    text = args[2].lower() if len(args) > 2 else ""
    stream = sys.argv[sys.argv.index("--stream") + 1] if "--stream" in sys.argv else ""
    limit = int(sys.argv[sys.argv.index("--max") + 1]) if "--max" in sys.argv else 10
    if "--stream" in sys.argv or "--max" in sys.argv:
        skip = {stream, str(limit)}
        args = [a for a in args if a not in skip]
        text = args[2].lower() if len(args) > 2 else ""
    found = 0
    for path in sorted(glob.glob(os.path.join(root, "**", "*.json"), recursive=True)):
        with open(path, encoding="utf-8") as f:
            g = json.load(f)
        if stream not in g["stream"]:
            continue
        res = int(os.path.basename(path).split("_")[0])
        for i, o in enumerate(g["objects"]):
            if (o.get("class_name") or o["class"]) != cls:
                continue
            attrs = []
            for p in o["packets"]:
                pn = p.get("class_name") or p["class"]
                for a in p["attrs"]:
                    v = a["value"]
                    if isinstance(v, list) and len(v) > 4:
                        v = "[%d numbers]" % len(v)
                    attrs.append("%s.%d=%s" % (pn, a["index"], v))
            line = " ".join(attrs)
            if text and text not in line.lower():
                continue
            print("%s  resource %d  object %d  guid %s\n    %s" % (g["stream"], res, i, o["guid"], line[:1500]))
            found += 1
            if found >= limit:
                return


if __name__ == "__main__":
    main()

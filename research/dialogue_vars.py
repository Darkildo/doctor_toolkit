#!/usr/bin/env python3
"""Which "variables" the face-to-face dialogues of the game test and set, and
what their choice nodes look like (stage 75).

Input: a directory made by `doctor-toolkit dialogue-export "$EARS_GF2_DIR" -o DIR`.

Usage: dialogue_vars.py DIR            counts per variable and per choice shape
       dialogue_vars.py DIR --choices  every choice node with its labels and what follows
"""
import collections
import glob
import json
import os
import re
import sys


def main():
    root = sys.argv[1]
    values = collections.defaultdict(collections.Counter)
    other = collections.Counter()
    shapes = collections.Counter()
    kinds = collections.Counter()
    for path in sorted(glob.glob(os.path.join(root, "*.json"))):
        with open(path, encoding="utf-8") as f:
            d = json.load(f)
        nodes = {n["id"]: n for n in d["nodes"]}
        for n in d["nodes"]:
            kinds[n["kind"]] += 1
            s = n.get("summary", "")
            m = re.search(r"var (0x[0-9a-f]+), (.*)", s)
            if m:
                values[(n["kind"], int(m.group(1), 16))][m.group(2)] += 1
            elif n["kind"] not in ("line", "choice", "end"):
                other[s] += 1
            if n["kind"] == "choice":
                c = {k: v for k, v in n.items() if k not in ("id", "kind", "children")}
                shapes[len(n["children"])] += 1
                if "--choices" in sys.argv:
                    after = [nodes[k]["summary"][:40] if k in nodes else "?" for k in n["children"]]
                    print(d["name"], n["id"], json.dumps(c, ensure_ascii=False)[:400], "->", after)
    if "--choices" in sys.argv:
        return
    print("node kinds:", dict(kinds))
    for (kind, var), c in sorted(values.items(), key=lambda x: (x[0][1], x[0][0])):
        print("  var 0x%02x  %-8s x%-4d %s" % (var, kind, sum(c.values()), dict(c.most_common(5))))
    print("other nodes:", dict(other.most_common(40)))
    print("choice nodes by number of branches:", dict(shapes))


if __name__ == "__main__":
    main()

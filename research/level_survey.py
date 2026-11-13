#!/usr/bin/env python3
"""Statistics behind the level view (stage 11, docs/levels.md).

Usage:
  level_survey.py metamodels <logic_root>
      <logic_root> holds `doctor-toolkit logic-export` JSON. Prints which
      part types reference which resource types through which properties,
      which state types own which parts, the LodState conditions and the
      nesting depth of metamodels.
  level_survey.py points <dump_root>
      <dump_root> holds `doctor-toolkit dump --type df14da75`. Walks the
      point records (16-byte prefix, 28 more bytes when its third word is
      not 0, then 0x7C bytes starting with a GUID) and checks that the walk
      finds the count at 0x04 and ends at the end of the data.

Checked claims (2026-11-12):
  - ModelPart parts have no matrix (15 553 of 15 553); nested models come
    from Entity.CMD_SetModel (Destructible, Door, BuildingWindow) and
    MetaModelPart.fileName with CMD_LoadMatrix or TransformedPart.Transform.
  - every part belongs to a state; all 427 LodStates have one condition
    with operator 2 and one with operator 3 on the variable `distance`.
  - metamodels nest at most 3 deep.
  - all 682 point tables walk to the end with exactly N records (52 162).
"""
import collections
import json
import os
import struct
import sys


def metamodels(root):
    refs = collections.Counter()
    owners = collections.Counter()
    lods = collections.Counter()
    nested = {}
    for dp, _, fns in os.walk(root):
        for fn in fns:
            if "_metamodel_" not in fn:
                continue
            d = json.load(open(os.path.join(dp, fn)))
            res = d["resources"]
            inner = set()
            for p in d["parts"]:
                for q in p["properties"]:
                    if q["type"] == "resource" and q["value"] is not None:
                        r = res[q["value"]]
                        kind = r.get("type_name", r["type_id"])
                        refs[(p["type"], q["class"] + "." + q["name"], kind)] += 1
                        if kind == "metamodel":
                            inner.add(r["id"])
                    if q["type"] == "matrix":
                        refs[(p["type"], "matrix", q["class"] + "." + q["name"])] += 1
            nested[d["id"]] = inner
            in_state = set()
            for s in d["states"]:
                in_state.update(s["parts"])
                for k in {d["parts"][i]["type"] for i in s["parts"]}:
                    owners[(s["type"], k)] += 1
                if s["type"] == "LodState":
                    ops = tuple(sorted(x["operator"] for x in s["conditions"] if x["variable"] == "distance"))
                    lods[ops] += 1
            owners[("parts outside states", len(d["parts"]) - len(in_state) > 0)] += 1

    def depth(m, seen=()):
        if m in seen or m not in nested:
            return 0
        return 1 + max((depth(n, seen + (m,)) for n in nested[m]), default=0)

    for k, v in refs.most_common(40):
        print(v, k)
    print("--- state types owning part types")
    for k, v in owners.most_common(30):
        print(v, k)
    print("--- LodState operators on `distance`:", dict(lods))
    print("--- nesting depth:", sorted(collections.Counter(depth(m) for m in nested).items()))


def points(root):
    c = collections.Counter()
    for dp, _, fns in os.walk(root):
        for fn in fns:
            d = open(os.path.join(dp, fn), "rb").read()
            n = struct.unpack_from("<I", d, 4)[0]
            o = 0x28
            bad = 0
            for _ in range(n):
                pre = struct.unpack_from("<4I", d, o)
                o += 16 + (28 if pre[2] else 0)
                if not all(48 <= b < 91 or b == 95 for b in d[o + 8:o + 16]):
                    bad += 1
                c[("kind", struct.unpack_from("<H", d, o + 0x78)[0])] += 1
                o += 0x7C
            c["records"] += n
            c[("ends at the end, GUIDs ok", o == len(d) and bad == 0)] += 1
    for k, v in c.most_common(12):
        print(v, k)


if __name__ == "__main__":
    {"metamodels": metamodels, "points": points}[sys.argv[1]](sys.argv[2])

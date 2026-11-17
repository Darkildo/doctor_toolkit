#!/usr/bin/env python3
"""Survey of the SimGroup classes that hand things to the player (stage 75).

Input: a directory made by
    doctor-toolkit logic-export "$EARS_GF2_DIR" -o DIR --type simgroup

Usage: items_survey.py DIR [class ...]
       items_survey.py DIR --refs     which attributes point at items and upgrades
       items_survey.py DIR --items    the `Item` objects by name, type and amount

For every class given (default: the classes of the stage) prints, per packet
and attribute, how often each value occurs; GUID attributes are resolved to
the class and the name (`Item` 0) of the object they point at.
"""
import collections
import glob
import json
import os
import sys

DEFAULT = ["MoneyAdaptor", "HonorAdaptor", "ScoreAdaptor", "StatAdaptor", "GiveItemRelay", "Bank",
           "RewardDestructible", "TestUpgrade", "UnlockCheck", "TestFamilySettings",
           "ChangeFamilySettings", "HireCrewRelay"]


def load(root):
    groups = []
    for path in sorted(glob.glob(os.path.join(root, "**", "*.json"), recursive=True)):
        with open(path, encoding="utf-8") as f:
            groups.append(json.load(f))
    return groups


def label(obj):
    name = ""
    for p in obj["packets"]:
        if p.get("class_name") in ("Item", "UpgradeData", "Family"):
            for a in p["attrs"]:
                if a["index"] == (1 if p["class_name"] == "Family" else 0):
                    name = str(a["value"])
    return "%s(%s)" % (obj.get("class_name") or obj["class"], name) if name else (obj.get("class_name") or obj["class"])


def refs(groups):
    by_guid = {}
    for g in groups:
        for o in g["objects"]:
            by_guid.setdefault(o["guid"], o)
    count = collections.Counter()
    example = {}
    for g in groups:
        for o in g["objects"]:
            for p in o["packets"]:
                for a in p["attrs"]:
                    t = by_guid.get(a["value"]) if a["type"] == "guid" else None
                    if t is None or t is o:
                        continue
                    tc = t.get("class_name")
                    if tc in ("UpgradeData", "Item", "Gun", "ThrowableItem"):
                        k = (o.get("class_name") or o["class"], p.get("class_name") or p["class"], a["index"], tc)
                        count[k] += 1
                        example.setdefault(k, (g["stream"], label(t)))
    for k, n in sorted(count.items(), key=lambda x: -x[1]):
        print("%5d  %s: %s.%d -> %s   e.g. %s %s" % ((n,) + k + example[k]))


def items(groups):
    count = collections.Counter()
    for g in groups:
        for o in g["objects"]:
            if o.get("class_name") not in ("Item", "Gun", "ThrowableItem"):
                continue
            d = {}
            for p in o["packets"]:
                if p.get("class_name") == "Item":
                    d = {a["index"]: a["value"] for a in p["attrs"]}
            count[(o["class_name"], d.get(0), d.get(3), d.get(4), d.get(9), d.get(30), d.get(14))] += 1
    print("count  class, name (0), type (3), amount (4), inventory flag (9), flags (30), message (14)")
    for k, n in sorted(count.items(), key=lambda x: (str(x[0][2]), str(x[0]))):
        print("%5d  %s" % (n, k))


def main():
    root = sys.argv[1]
    groups = load(root)
    if "--refs" in sys.argv:
        return refs(groups)
    if "--items" in sys.argv:
        return items(groups)
    wanted = sys.argv[2:] or DEFAULT
    by_guid = {}
    for g in groups:
        for o in g["objects"]:
            by_guid.setdefault(o["guid"], o)
    for cls in wanted:
        stats = collections.defaultdict(collections.Counter)
        count = 0
        streams = collections.Counter()
        for g in groups:
            for o in g["objects"]:
                if (o.get("class_name") or o["class"]) != cls:
                    continue
                count += 1
                streams[g["stream"]] += 1
                for p in o["packets"]:
                    pn = p.get("class_name") or p["class"]
                    if pn in ("CAttributeHandler", "Base", "CSystemCommands", "Entity"):
                        continue
                    for a in p["attrs"]:
                        v = a["value"]
                        if a["type"] == "guid":
                            t = by_guid.get(v)
                            v = label(t) if t else ("res:" + a["resource"] if "resource" in a else "guid?")
                        elif isinstance(v, (list, dict)):
                            v = json.dumps(v)[:40]
                        stats[(pn, a["index"], a["type"])][v] += 1
        print("== %s: %d objects in %d streams" % (cls, count, len(streams)))
        for (pn, i, ty), c in sorted(stats.items()):
            top = ", ".join("%s x%d" % (str(v)[:48], n) for v, n in c.most_common(14))
            print("  %s.%d %s [%d distinct]: %s" % (pn, i, ty, len(c), top))


if __name__ == "__main__":
    main()

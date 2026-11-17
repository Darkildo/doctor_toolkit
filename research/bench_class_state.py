#!/usr/bin/env python3
"""Compares what the code of the logic classes says about the state of their
objects with a running game (stage 79, docs/analysis/game-bench.md §41).

Usage:
    bench_class_state.py table <simgroup_dump_dir> > objects.tsv
        GUID, class and the attributes that matter of every MultiManager,
        TriggerAuto, VariableSwitch, VariableWatcher, TriggerBox and
        EnterExitTrigger of the game files (the dump of
        `doctor-toolkit dump --type simgroup`).
    bench_class_state.py report objects.tsv world.json [streams.txt]
        world.json is `doctor-toolkit live objects --all --json` of the running
        game. Prints, per class, how many loaded objects have their message
        listener switched on (bit 0 of the listener word +0x40) against what
        the code predicts:
          TriggerAuto      off (fires once on iMsgPostSpawnInitialize and
                           switches its listener off, 0x00477EF0)
          MultiManager     on while the count of attribute 1 is not 0
                           (0x0047A1C0 switches it off at 0)
        With streams.txt (`doctor-toolkit live streams`): per class, how many
        objects the SimGroups of the loaded streams hold and how many of them
        the game has; an object that switched itself off is removed.
    bench_class_state.py pick objects.tsv world.json
        A loaded MultiManager with attribute 1 = 1 that still listens: GUID,
        trigger message and its outputs, for the one-shot experiment.
"""
import json
import os
import struct
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
from class_usage import groups, packet, rel, sdbm  # noqa: E402

CLASSES = ["MultiManager", "TriggerAuto", "VariableSwitch", "VariableWatcher", "TriggerBox", "EnterExitTrigger"]


def table(root):
    want = {sdbm(c): c for c in CLASSES}
    for path, d, offs, (aux, strs, pk) in groups(root):
        stream = os.path.relpath(os.path.dirname(path), root).replace(os.sep, "/").lower()
        for o in offs:
            cls = struct.unpack_from("<I", d, o + 0x28)[0]
            if cls not in want:
                continue
            guid = d[rel(d, o + 0x24):rel(d, o + 0x24) + 16].hex()
            ng, npk = d[o + 0x1E], d[o + 0x1F]
            vals = {}
            for k in range(ng, ng + npk):
                h, slots = packet(d, rel(d, o + 0x2C + 4 * k))
                for i, q in slots.items():
                    raw = struct.unpack_from("<I", d, q)[0]
                    t = q + struct.unpack_from("<i", d, q)[0]
                    if raw and strs <= t < pk:
                        vals[(h, i)] = d[t:d.index(b"\0", t)].decode("latin-1")
                    else:
                        vals[(h, i)] = struct.unpack("<i", struct.pack("<I", raw))[0]
            name = want[cls]
            if name == "MultiManager":
                h = cls
                sends = [str(vals.get((h, 3 + 2 * k), "")) for k in range(8)]
                row = [vals.get((h, 0), ""), vals.get((h, 1), 0), vals.get((h, 2), 0), ",".join(s for s in sends if s)]
            elif name == "TriggerAuto":
                row = [vals.get((cls, 0), ""), "", vals.get((cls, 2), 0), ""]
            elif name == "TriggerBox":
                h = sdbm("TriggerBase")
                row = ["", vals.get((h, 6), 0), vals.get((h, 8), 0), ""]
            elif name == "EnterExitTrigger":
                row = ["", "", vals.get((cls, 6), 0), ""]
            else:
                row = [vals.get((cls, 0), ""), "", "", ""]
            print("\t".join([guid, name] + [str(x) for x in row] + [stream]))


def load(tsv, world):
    rows = {}
    for line in open(tsv):
        p = line.rstrip("\n").split("\t")
        rows[p[0]] = p
    return rows, json.load(open(world))["objects"]


def kind(r):
    if r[1] == "MultiManager":
        return "MultiManager (count %s)" % ("-1" if r[3] == "-1" else "0" if r[3] == "0" else "> 0")
    return r[1]


def report(tsv, world, streams=None):
    rows, objects = load(tsv, world)
    if streams:
        # Objects the files of the loaded streams hold against the objects
        # the game has: a class whose objects are gone removed them.
        loaded = set()
        for line in open(streams):
            name = line.strip().replace("\\", "/").lower()
            if name.endswith(".str"):
                loaded.add(name[:-4])
        alive = {o["guid"] for o in objects}
        tally = {}
        for r in rows.values():
            if r[6] in loaded:
                a, b = tally.get(kind(r), (0, 0))
                tally[kind(r)] = (a + 1, b + (r[0] in alive))
        print("loaded streams: %d" % len(loaded))
        for k in sorted(tally):
            print("%-28s in the files of loaded streams %5d   objects in the game %5d" % (k, *tally[k]))
    count = {}
    for o in objects:
        r = rows.get(o["guid"])
        if not r:
            continue
        key = kind(r)
        on, off = count.get(key, (0, 0))
        count[key] = (on + bool(o["listening"]), off + (not o["listening"]))
    print("loaded objects: %d; of the classes of the table: %d" % (len(objects), sum(a + b for a, b in count.values())))
    for key in sorted(count):
        print("%-28s listening %5d   not listening %5d" % (key, *count[key]))


def pick(tsv, world):
    rows, objects = load(tsv, world)
    for o in objects:
        r = rows.get(o["guid"])
        if r and r[1] == "MultiManager" and r[3] == "1" and r[4] in ("0", "4") and o["listening"] and r[2] and r[5]:
            print("\t".join([r[0], r[2], r[5]]))


if __name__ == "__main__":
    a = sys.argv[1:]
    if len(a) == 2 and a[0] == "table":
        table(a[1])
    elif len(a) in (3, 4) and a[0] == "report":
        report(*a[1:])
    elif len(a) == 3 and a[0] == "pick":
        pick(a[1], a[2])
    else:
        sys.exit(__doc__)

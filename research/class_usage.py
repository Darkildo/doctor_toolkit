#!/usr/bin/env python3
"""Counts the objects of every SimGroup class in the game files.

Usage:
    class_usage.py <simgroup_dump_dir> [top N]
    class_usage.py <simgroup_dump_dir> values <Class or packet hash> [attribute index]

The dump is made with `doctor-toolkit dump --type simgroup -o <dir> GAME`.
The first form prints, most used first: objects, SimGroups that have the
class, class name (data/gf2/sim_classes.txt; the hash when it has no name) and
the category of data/gf2/logic_schema.toml.

The second form prints the values of the attributes of one packet (the class
packet, e.g. `TriggerBase`) across the game: for each attribute the number of
objects that store a value and the most common values (as integer, float and
text when the value points at a string).
"""
import collections
import os
import re
import struct
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
DATA = os.path.join(HERE, "..", "data", "gf2")


def sdbm(s):
    h = 0
    for ch in s.lower().encode():
        h = (ch + (h << 6) + (h << 16) - h) & 0xFFFFFFFF
    return h


def names():
    out = {}
    for line in open(os.path.join(DATA, "sim_classes.txt")):
        line = line.strip()
        if line and not line.startswith("#"):
            out[sdbm(line)] = line
    return out


def categories():
    out = {}
    on = False
    for line in open(os.path.join(DATA, "logic_schema.toml")):
        if line.startswith("["):
            on = line.strip() == "[class]"
            continue
        m = re.match(r'(\w+) = "(\w+)"', line)
        if on and m:
            out[m.group(1)] = m.group(2)
    return out


def rel(d, q):
    return q + struct.unpack_from("<i", d, q)[0]


def packet(d, p):
    """(hash, {attribute index: position of the value})"""
    h, na = struct.unpack_from("<IH", d, p)
    mb = (na + 7) // 8
    mask = int.from_bytes(d[p + 6:p + 6 + mb], "little")
    hl = (6 + mb + 3) & ~3
    vals = [i for i in range(na) if not (mask >> i) & 1]
    return h, {i: p + hl + 4 * k for k, i in enumerate(vals)}


def groups(root):
    for dp, _, fns in sorted(os.walk(root)):
        for fn in sorted(fns):
            path = os.path.join(dp, fn)
            d = open(path, "rb").read()
            n, _, auxsz, strsz, tbl = struct.unpack_from("<5I", d, 0x18)
            aux = tbl + 4 * n
            strs = aux + auxsz
            yield path, d, struct.unpack_from("<%dI" % n, d, tbl), (aux, strs, strs + strsz)


def objects(d, offs):
    """(class hash, [packet positions]) for every object"""
    for o in offs:
        ng, npk = d[o + 0x1E], d[o + 0x1F]
        cls = struct.unpack_from("<I", d, o + 0x28)[0]
        yield cls, [rel(d, o + 0x2C + 4 * k) for k in range(ng, ng + npk)]


def usage(root, top):
    nm, cat = names(), categories()
    objs, files = collections.Counter(), collections.Counter()
    for _, d, offs, _ in groups(root):
        seen = set()
        for cls, _ in objects(d, offs):
            objs[cls] += 1
            seen.add(cls)
        for cls in seen:
            files[cls] += 1
    print("# objects\tsimgroups\tclass\tcategory")
    for cls, n in objs.most_common(top):
        name = nm.get(cls, "%08x" % cls)
        print("%d\t%d\t%s\t%s" % (n, files[cls], name, cat.get(name, "-")))
    print("# classes: %d, objects: %d" % (len(objs), sum(objs.values())), file=sys.stderr)


def values(root, what, only):
    h = int(what, 16) if re.fullmatch(r"[0-9a-fA-F]{8}", what) else sdbm(what)
    seen = collections.defaultdict(collections.Counter)
    total = 0
    for _, d, offs, (aux, strs, pk) in groups(root):
        for _, pks in objects(d, offs):
            for p in pks:
                ph, slots = packet(d, p)
                if ph != h:
                    continue
                total += 1
                for i, q in slots.items():
                    raw = struct.unpack_from("<I", d, q)[0]
                    t = q + struct.unpack_from("<i", d, q)[0]
                    if raw and strs <= t < pk:
                        v = '"%s"' % d[t:d.index(b"\0", t)].decode("latin-1")
                    elif raw and aux <= t < strs:
                        v = "aux:" + d[t:t + 16].hex()
                    else:
                        v = "%d|0x%x|%g" % (struct.unpack("<i", struct.pack("<I", raw))[0], raw,
                                             struct.unpack("<f", struct.pack("<I", raw))[0])
                    seen[i][v] += 1
    print("# packet %08x: %d objects" % (h, total))
    for i in sorted(seen):
        if only is not None and i != only:
            continue
        c = seen[i]
        n = 40 if only is not None else 6
        print("%d\tstored %d, distinct %d\t%s" % (
            i, sum(c.values()), len(c), "; ".join("%s x%d" % kv for kv in c.most_common(n))))


if __name__ == "__main__":
    if len(sys.argv) >= 4 and sys.argv[2] == "values":
        values(sys.argv[1], sys.argv[3], int(sys.argv[4]) if len(sys.argv) > 4 else None)
    elif len(sys.argv) >= 2:
        usage(sys.argv[1], int(sys.argv[3]) if len(sys.argv) > 3 and sys.argv[2] == "top" else None)
    else:
        sys.exit(__doc__)

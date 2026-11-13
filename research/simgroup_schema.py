#!/usr/bin/env python3
"""Infers the value kind of every SimGroup attribute from the game files.

Usage: simgroup_schema.py <simgroup_dump_dir> [<metamodel_dump_dir>] > data/gf2/simgroup_schema.txt

Attributes are stored as one u32 per value. A value is either a scalar or a
self-relative pointer into the string region or the aux region (see
simgroup_survey.py). For every (packet class, attribute index) this script
collects all values in the corpus and decides:

- string: the values point into the string region (names, event ids);
- guid / vec3 / matrix: the values point into the aux region; the size is the
  most common distance to the next referenced aux blob (16, 12, 64 bytes).
  Equal blobs are stored once and may overlap, so other distances occur;
- float: every value is a plausible single-precision number, not a small integer;
- int: every value is a small signed integer;
- hash: anything else (32-bit hashes and bit masks).

Attribute names come from metamodel resources: their property table stores
(class name, attribute index, attribute name) for the attributes a metamodel
sets, e.g. (`CSystemCommands`, 1, `CMD_LoadMatrix`).

Output lines: `<class hash> <attribute index> <kind> <number of values seen> [<name>]`.
"""
import collections
import os
import struct
import sys

from simgroup_survey import packets, rel

AUX_KINDS = {16: "guid", 12: "vec3", 64: "matrix"}


def plausible_float(v):
    f = struct.unpack("<f", struct.pack("<I", v))[0]
    if f != f or f in (float("inf"), float("-inf")):
        return False
    return 1e-6 <= abs(f) <= 1e7


def small_int(v):
    i = v - (1 << 32) if v & 0x80000000 else v
    return -0x10000 <= i <= 0x100000


def sdbm_lower(s):
    h = 0
    for c in s.lower().encode():
        h = (c + h * 65599) & 0xFFFFFFFF
    return h


def metamodel_names(root):
    """(class hash, attribute index) -> attribute name, from metamodel property tables."""
    names = {}
    for dp, _, fns in os.walk(root):
        for fn in fns:
            d = open(os.path.join(dp, fn), "rb").read()
            table = struct.unpack_from("<I", d, 0x34 + 4 * 3)[0]
            count = struct.unpack_from("<H", d, 0x58 + 2 * 3)[0]
            for k in range(count):
                _, name, cls, idx, _, _ = struct.unpack_from("<6I", d, table + 0x18 * k)
                name = d[name:d.index(b"\0", name)].decode()
                cls = d[cls:d.index(b"\0", cls)].decode()
                names[(sdbm_lower(cls), idx)] = name
    return names


def main(root, metamodel_root=None):
    names = metamodel_names(metamodel_root) if metamodel_root else {}
    kinds = collections.defaultdict(collections.Counter)
    scalars = collections.defaultdict(list)
    gaps = collections.defaultdict(collections.Counter)
    for dp, _, fns in os.walk(root):
        for fn in fns:
            d = open(os.path.join(dp, fn), "rb").read()
            n, _, auxsz, strsz, tbl = struct.unpack_from("<5I", d, 0x18)
            aux = tbl + 4 * n
            strs = aux + auxsz
            pk = strs + strsz
            offs = struct.unpack_from("<%dI" % n, d, tbl)
            units, _ = packets(d, pk, offs[0] if n else len(d))
            targets = collections.defaultdict(set)
            for o in offs:
                targets[rel(d, o + 0x24)].add(None)
                for k in range(d[o + 0x1E]):
                    targets[rel(d, o + 0x2C + 4 * k)].add(None)
            for h, _, _, slots, idx in units.values():
                for q, i in zip(slots, idx):
                    v = struct.unpack_from("<I", d, q)[0]
                    t = (q + v) & 0xFFFFFFFF
                    if aux <= t < strs:
                        kinds[(h, i)]["aux"] += 1
                        targets[t].add((h, i))
                    elif strs <= t < pk:
                        kinds[(h, i)]["string"] += 1
                    else:
                        kinds[(h, i)]["scalar"] += 1
                        scalars[(h, i)].append(v)
            ts = sorted(targets) + [strs]
            for a, b in zip(ts, ts[1:]):
                for key in targets[a]:
                    if key is not None:
                        gaps[key][b - a] += 1
    print("# SimGroup attribute value kinds, inferred from the game files by")
    print("# research/simgroup_schema.py. Columns: class hash, attribute index, kind,")
    print("# number of non-default values seen in the corpus, attribute name when a")
    print("# metamodel names it.")
    for key in sorted(kinds):
        c = kinds[key]
        total = sum(c.values())
        top, top_n = c.most_common(1)[0]
        if top == "aux":
            sizes = collections.Counter({s: n for s, n in gaps[key].items() if s in AUX_KINDS})
            if sizes:
                kind = AUX_KINDS[sizes.most_common(1)[0][0]]
            else:
                # Blobs are deduplicated and may overlap: take the smallest kind
                # that covers the largest distance seen.
                kind = AUX_KINDS[min(s for s in AUX_KINDS if s >= max(gaps[key]))]
        elif top == "string":
            kind = "string"
        else:
            vals = scalars[key]
            if all(small_int(v) for v in vals):
                kind = "int"
            elif all(plausible_float(v) or v == 0 for v in vals):
                kind = "float"
            else:
                kind = "hash"
        name = names.get(key)
        print("%08x %d %s %d" % (key[0], key[1], kind, total) + (" " + name if name else ""))


if __name__ == "__main__":
    main(*sys.argv[1:3])

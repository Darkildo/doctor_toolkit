#!/usr/bin/env python3
"""Recovers SimGroup class names by hashing candidate strings.

Usage: sim_class_names.py <godfather2.exe> <simgroup_dump_dir> [<other dump dirs>...] > data/gf2/sim_classes.txt

A SimGroup attribute packet and an object are tagged with the SDBM hash of the
lower-case class name (`sdbm("entity") == 0x38523FC3`). Candidates are:

1. identifiers found in the executable and in the dumped resources;
2. concatenations of up to three CamelCase words taken from the names found
   in step 1, optionally prefixed with `C` (`CAttributeHandler`).

Only names whose hash occurs in the SimGroup files are printed.
"""
import collections
import os
import re
import struct
import sys

from simgroup_survey import packets

M = 0xFFFFFFFF
ACRONYMS = {"AI", "CSA", "NPC", "TOD", "UI", "MM", "VFX", "LUT", "LOD", "GUID", "F2F", "SH", "POI"}
EXTRA_WORDS = ["Base", "Component", "Handler", "Attribute", "Manager", "Filter", "Requester"]


def sdbm(s, h=0):
    for c in s.lower().encode():
        h = (c + h * 65599) & M
    return h


def identifiers(blob):
    for t in re.findall(rb"[A-Za-z_][A-Za-z0-9_:]{2,}", blob):
        t = t.decode()
        yield t
        yield from t.split("::")
        yield from t.split("_")
        yield re.sub(r"[_0-9]+$", "", t)


def wanted_hashes(simgroup_dir):
    want = collections.Counter()
    for dp, _, fns in os.walk(simgroup_dir):
        for fn in fns:
            d = open(os.path.join(dp, fn), "rb").read()
            n, _, auxsz, strsz, tbl = struct.unpack_from("<5I", d, 0x18)
            offs = struct.unpack_from("<%dI" % n, d, tbl)
            units, _ = packets(d, tbl + 4 * n + auxsz + strsz, offs[0] if n else len(d))
            for h, _, _, _, _ in units.values():
                want[h] += 1
            for o in offs:
                want[struct.unpack_from("<I", d, o + 0x28)[0]] += 1
                comp = struct.unpack_from("<i", d, o + 0x20)[0]
                if comp:
                    # The component packet holds the class hashes of the components.
                    for q in units[o + 0x20 + comp][3]:
                        want[struct.unpack_from("<I", d, q)[0]] += 1
    return want


def camel(name):
    """`BASE` -> `Base`, `entity` -> `Entity`; acronyms stay upper case."""
    if name.upper() in ACRONYMS:
        return name.upper()
    if name.isupper() or name.islower():
        return name[0].upper() + name[1:].lower()
    return name[0].upper() + name[1:]


def words_of(name):
    return [camel(w) for w in re.findall(r"[A-Z]+(?=[A-Z][a-z])|[A-Z]?[a-z]+|[A-Z]+|\d+", name)]


def main(exe, simgroup_dir, others):
    want = wanted_hashes(simgroup_dir)
    found = {}

    def offer(name):
        h = sdbm(name)
        if h in want:
            old = found.get(h)
            # Prefer mixed-case spellings ("Entity" over "entity" or "ENTITY").
            if old is None or (old.islower() or old.isupper()) and not (name.islower() or name.isupper()):
                found[h] = name

    for t in identifiers(open(exe, "rb").read()):
        offer(t)
    for root in [simgroup_dir] + others:
        for dp, _, fns in os.walk(root):
            for fn in fns:
                for t in identifiers(open(os.path.join(dp, fn), "rb").read()):
                    offer(t)
    for h, name in list(found.items()):
        if name.upper() in ACRONYMS:
            found[h] = name.upper()
        elif name.islower() or name.isupper():
            found[h] = "".join(words_of(name)) if name.upper() not in ACRONYMS else name.upper()
    vocab = sorted({w for name in found.values() for w in words_of(name)} | set(EXTRA_WORDS))
    missing = {h for h in want if h not in found}
    for prefix in ("", "C"):
        for a in vocab:
            ha = sdbm(prefix + a)
            for b in vocab:
                hb = sdbm(b, ha)
                if hb in missing:
                    found[hb] = prefix + a + b
                    missing.discard(hb)
                for c in vocab:
                    hc = sdbm(c, hb)
                    if hc in missing:
                        found[hc] = prefix + a + b + c
                        missing.discard(hc)
    print("# SimGroup class names of The Godfather II (PC).")
    print("# A class is identified by the SDBM hash of its lower-case name.")
    print("# Recovered by research/sim_class_names.py: hashing identifiers from the")
    print("# executable and the game files, and combinations of their words.")
    for name in sorted(found.values(), key=str.lower):
        print(name)
    print("%d of %d class hashes named" % (len(found), len(want)), file=sys.stderr)


if __name__ == "__main__":
    main(sys.argv[1], sys.argv[2], sys.argv[3:])

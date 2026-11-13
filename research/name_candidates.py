#!/usr/bin/env python3
"""Confirms candidate names against the hashes the game actually contains.

Usage:
    doctor-toolkit unknown-hashes "$GAME" --list 1000000 > unknown.txt
    name_candidates.py unknown.txt <candidate files>... [options] > confirmed.tsv

A candidate is accepted only when its hash, under the function the engine uses
for that kind of name, equals a hash that occurs in the game and has no name
yet. Everything else is dropped, so any list of strings can be fed in: name
lists of other tools, strings of the executable, dumps, notes.

Every line of a candidate file gives several candidates: the whole line, the
words in it (runs of letters, digits, `_`, `.`, `-`, `\\`, `/`), the
components of a word that is a path, and every component without its
extensions (`chars\\cct\\export\\bouncer.bnk.xen` gives `bouncer.bnk.xen` and
`bouncer`). JSON and source code work as they are.

Options:
    --case             also try the lower-case, upper-case and capitalised form
    --prefix TEXT      also try TEXT + candidate (repeatable)
    --suffix TEXT      also try candidate + TEXT (repeatable; `_lod1`, `_01`)
    --kind KIND        only this kind (as printed by `unknown-hashes`)
    --any-function     also try every function on kinds whose function is not
                       known (resource ids); such matches are printed with `?`
                       and are hints, not confirmed names

Output, one confirmed pair per line, most frequent first within a kind:
    kind <TAB> hash <TAB> function <TAB> places <TAB> name
A hash that several candidates give is printed once per candidate. For
functions of the lower-case name every spelling is printed: pick the one the
documents use. A summary per kind goes to stderr.

Functions per kind (docs/navigation.md §2): type tags, bones and the ids of
`smb` and `sbk` resources are SDBM of the name as written (the ids: of the
build path, `levels\\gf2\\cuba\\streams\\cuba\\local.smb`); SimGroup classes,
string entries and metamodel names are SDBM of the lower-case name; shaders
are CRC-32 of the lower-case name.
"""
import collections
import re
import sys
import zlib

M = 0xFFFFFFFF


def sdbm(text):
    h = 0
    for c in text.encode("utf-8", "surrogateescape"):
        h = (c + h * 65599) & M
    return h


def fnv1a(text):
    h = 0x811C9DC5
    for c in text.encode("utf-8", "surrogateescape"):
        h = ((h ^ c) * 0x01000193) & M
    return h


FUNCTIONS = {
    "sdbm": sdbm,
    "sdbm(lower)": lambda s: sdbm(s.lower()),
    "crc32": lambda s: zlib.crc32(s.encode("utf-8", "surrogateescape")),
    "crc32(lower)": lambda s: zlib.crc32(s.lower().encode("utf-8", "surrogateescape")),
    "fnv1a": fnv1a,
    "fnv1a(lower)": lambda s: fnv1a(s.lower()),
}

KIND_FUNCTIONS = {
    "type tag": ["sdbm"],
    "bone": ["sdbm(lower)"],
    "smb id": ["sdbm"],
    "sbk id": ["sdbm"],
    "SimGroup class": ["sdbm(lower)"],
    "string entry": ["sdbm(lower)"],
    "metamodel name": ["sdbm(lower)"],
    "shader": ["crc32(lower)"],
}

WORD = re.compile(r"[A-Za-z0-9_.\-\\/]+")


def read_unknown(path):
    """`{kind: {hash: places}}` from the output of `unknown-hashes --list N`."""
    unknown = collections.defaultdict(dict)
    kind = None
    with open(path, encoding="utf-8") as f:
        for line in f:
            if line.startswith("    ") and kind:
                h, places = line.split()
                unknown[kind][int(h, 16)] = int(places)
            elif "unknown hash" in line:
                kind = line.split(":")[0]
            else:
                kind = None
    return unknown


def candidates_of(line):
    line = line.strip()
    if not line:
        return
    yield line
    for word in WORD.findall(line):
        word = word.strip(".-").replace("\\\\", "\\")
        if not word:
            continue
        yield word
        for part in re.split(r"[\\/]", word):
            if part:
                yield part
                yield part.split(".")[0]


def read_candidates(paths, case, prefixes, suffixes):
    names = set()
    for path in paths:
        with open(path, encoding="utf-8", errors="surrogateescape") as f:
            for line in f:
                names.update(candidates_of(line))
    if case:
        names |= {f(n) for n in list(names) for f in (str.lower, str.upper, str.capitalize)}
    extra = set()
    for n in names:
        for p in prefixes:
            extra.add(p + n)
        for s in suffixes:
            extra.add(n + s)
            for p in prefixes:
                extra.add(p + n + s)
    return names | extra


def main(argv):
    args = argv[1:]
    opts = {"case": False, "any": False, "kind": None, "prefix": [], "suffix": []}
    files = []
    i = 0
    while i < len(args):
        a = args[i]
        if a == "--case":
            opts["case"] = True
        elif a == "--any-function":
            opts["any"] = True
        elif a in ("--prefix", "--suffix", "--kind"):
            i += 1
            if a == "--kind":
                opts["kind"] = args[i]
            else:
                opts[a[2:]].append(args[i])
        elif a.startswith("--"):
            sys.exit(f"unknown option {a}")
        else:
            files.append(a)
        i += 1
    if len(files) < 2:
        sys.exit(__doc__)
    unknown = read_unknown(files[0])
    names = read_candidates(files[1:], opts["case"], opts["prefix"], opts["suffix"])
    print(f"{len(names)} candidate(s)", file=sys.stderr)

    # Hash every candidate once per function used by any wanted kind.
    wanted = {}
    for kind, hashes in unknown.items():
        if opts["kind"] and kind != opts["kind"]:
            continue
        fns = KIND_FUNCTIONS.get(kind)
        confirmed = fns is not None
        if fns is None:
            if not opts["any"]:
                continue
            fns = list(FUNCTIONS)
        wanted[kind] = (hashes, fns, confirmed)
    used = sorted({f for _, fns, _ in wanted.values() for f in fns})
    by_hash = {f: collections.defaultdict(set) for f in used}
    for n in names:
        for f in used:
            by_hash[f][FUNCTIONS[f](n)].add(n)

    for kind in sorted(wanted):
        hashes, fns, confirmed = wanted[kind]
        found = []
        for h, places in hashes.items():
            for f in fns:
                for n in sorted(by_hash[f].get(h, ())):
                    found.append((places, h, f, n))
        found.sort(key=lambda x: (-x[0], x[1], x[3]))
        mark = "" if confirmed else "?"
        for places, h, f, n in found:
            print(f"{kind}\t{h:08x}\t{f}{mark}\t{places}\t{n}")
        distinct = {h for _, h, _, _ in found}
        total = sum(hashes.values())
        got = sum(hashes[h] for h in distinct)
        print(
            f"{kind}: {len(distinct)} of {len(hashes)} hash(es), "
            f"{got} of {total} place(s){'' if confirmed else ' (function not known: hints only)'}",
            file=sys.stderr,
        )


if __name__ == "__main__":
    main(sys.argv)

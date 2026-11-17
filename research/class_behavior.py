#!/usr/bin/env python3
"""Checks data/gf2/class_behavior.toml (what SimGroup classes do, read in the
code of godfather2.exe) against the other data files and the game files.

Usage:
    class_behavior.py stats                 counts for reports
    class_behavior.py kinds                 value kinds by code against the kinds
                                            inferred from the game files
                                            (data/gf2/simgroup_schema.txt)
    class_behavior.py types                 attribute meanings of
                                            data/gf2/attr_types.toml that the
                                            code confirms or corrects
    class_behavior.py corpus <dump_dir>     checks on the game files (the dump of
                                            `doctor-toolkit dump --type simgroup`)
    class_behavior.py tables <class_dump_dir>
                                            readers and handlers of the file against
                                            the method tables (the output directory
                                            of research/ghidra/ClassDump.java)
    class_behavior.py schema                puts the attribute names and message
                                            pins of the file into
                                            data/gf2/logic_schema.toml (in place;
                                            running it again changes nothing)

`corpus` checks what the code says about the stored form of an object
(0x00446BF0): bit 1 of the flag byte 0x1C of an object is set exactly when
the object has a Base packet, and byte 0x1D is the value of attribute 0 of
that packet (0 when it is not stored). It exits with 1 on a mismatch.

The file itself is kept by hand: the code is read with the Ghidra scripts of
research/ghidra (ClassDump.java gives the constructors, method tables and
methods of a class; README there).
"""
import collections
import os
import re
import struct
import sys
import tomllib

HERE = os.path.dirname(os.path.abspath(__file__))
DATA = os.path.join(HERE, "..", "data", "gf2")
sys.path.insert(0, HERE)
from class_usage import groups, objects, packet, sdbm  # noqa: E402

# kind by code -> kinds of simgroup_schema.txt it may be stored as
FITS = {
    "msg_in": {"string"}, "msg_out": {"string"}, "string": {"string"},
    "guid": {"guid"}, "matrix": {"matrix"}, "vec3": {"vec3"},
    "float": {"float"}, "int": {"int"}, "flags": {"int"}, "enum": {"int"}, "bool": {"int"},
    # a hash is stored as a number, or as text the reader hashes
    "hash": {"hash", "int", "string"},
}


def phash(name):
    return int(name, 16) if re.fullmatch(r"[0-9a-fA-F]{8}", name) else sdbm(name)


def load():
    return tomllib.load(open(os.path.join(DATA, "class_behavior.toml"), "rb"))["class"]


def attrs(classes):
    for name, c in classes.items():
        for a in c.get("attr", []):
            yield name, a


def data_kinds():
    out = {}
    for line in open(os.path.join(DATA, "simgroup_schema.txt")):
        p = line.split()
        if p and not line.startswith("#"):
            out[(int(p[0], 16), int(p[1]))] = p[2]
    return out


# Packets every placed object has: their attributes are not pins of the graph.
COMMON = {"Base", "CSystemCommands"}
ATTR_LINE = re.compile(r"^  \{ (i = \d+.*) \},$")
FIELD = re.compile(r'(\w+) = ("[^"]*"|\d+)')
ORDER = ["i", "pin", "conf", "name", "name_conf", "delay", "kind"]


def apply_schema(text):
    """Puts the attribute names and message pins read in the code into the
    text of data/gf2/logic_schema.toml: the name of an attribute (with its
    confidence) and, for an attribute that names a message, its direction
    with confidence `exe`. Attributes and packets the schema lacks are
    added (parameters, or message pins). Links, resources, delays and kinds
    stay. research/logic_schema.py calls this on what it generates;
    `class_behavior.py schema` rewrites the file in place."""
    dk = data_kinds()
    want = collections.defaultdict(dict)  # packet hash -> index -> attribute
    names = {}
    for _, a in attrs(load()):
        if a["packet"] in COMMON:
            continue
        h = phash(a["packet"])
        want[h][a["i"]] = a
        names[h] = a["packet"]

    def line(fields):
        return "  { %s },\n" % ", ".join("%s = %s" % (k, fields[k]) for k in ORDER if k in fields)

    def merged(fields, a, h):
        fields = dict(fields)
        fields["name"] = '"%s"' % a["name"]
        fields["name_conf"] = '"%s"' % a["conf"]
        pin = {"msg_in": "in", "msg_out": "out"}.get(a["kind"])
        if pin:
            fields["pin"], fields["conf"] = '"%s"' % pin, '"exe"'
        fields.setdefault("pin", '"param"')
        if "kind" not in fields and (h, a["i"]) in dk:
            fields["kind"] = '"%s"' % dk[(h, a["i"])]
        return fields

    lines = text.splitlines(keepends=True)
    out = []
    k = 0
    done = set()
    while k < len(lines):
        m = re.match(r"\[packet\.(.+)\]$", lines[k].strip())
        if not m:
            out.append(lines[k])
            k += 1
            continue
        h = int(re.search(r'"([0-9a-f]{8})"', lines[k + 1]).group(1), 16)
        out += lines[k:k + 3]  # header, hash, `attrs = [`
        k += 3
        rows = {}
        while not lines[k].startswith("]"):
            fields = dict(FIELD.findall(ATTR_LINE.match(lines[k]).group(1)))
            rows[int(fields["i"])] = fields
            k += 1
        for i, a in want.get(h, {}).items():
            rows[i] = merged(rows.get(i, {"i": str(i)}), a, h)
        done.add(h)
        out += [line(rows[i]) for i in sorted(rows)]
    text = "".join(out)
    # Packets the schema did not have, in the order of the file (by name).
    for h in sorted(set(want) - done, key=lambda h: names[h]):
        rows = [line(merged({"i": str(i)}, a, h)) for i, a in sorted(want[h].items())]
        block = "[packet.%s]\nhash = \"%08x\"\nattrs = [\n%s]\n\n" % (names[h], h, "".join(rows))
        at = len(text)
        for m in re.finditer(r"(?m)^\[packet\.\"?([^\]\"]+)\"?\]$", text):
            if m.group(1) > names[h]:
                at = m.start()
                break
        text = text[:at] + block + text[at:]
    return text


def tables(root):
    """Compares `reader` and `handler` of the file with the method tables
    of the classes: <root>/<Class>/index.txt as research/ghidra/ClassDump.java
    writes it. The reader is slot 1 of the table at +0x0, the handler slot 1
    of the listener table at +0x3c of the class's own constructor."""
    classes = load()
    bad = same = 0
    for name in sorted(os.listdir(root)):
        index = os.path.join(root, name, "index.txt")
        if name not in classes or not os.path.exists(index):
            continue
        slots = {}
        for line in open(index):
            if line.startswith("ctor[1]"):
                break
            m = re.match(r"\s+table \+0x([0-9a-f]+) [0-9a-f]+:(.*)", line)
            if m:
                slots[int(m.group(1), 16)] = dict(
                    (int(k), v.rstrip("*").upper()) for k, v in (x.split(":") for x in m.group(2).split()))
        for key, off in (("reader", 0), ("handler", 0x3C)):
            have = classes[name].get(key)
            slot = slots.get(off, {}).get(1)
            if not have or not slot:
                print("%-24s %-7s file %s, table %s: not compared" % (name, key, have, slot))
            elif have[2:] == slot:
                same += 1
            else:
                bad += 1
                print("%-24s %-7s file %s, table slot 0x%s" % (name, key, have, slot))
    print("agree: %d, differ: %d" % (same, bad))


def stats():
    classes = load()
    schema = tomllib.load(open(os.path.join(DATA, "logic_schema.toml"), "rb"))["packet"]
    old = {(int(p["hash"], 16), a["i"]): a for p in schema.values() for a in p["attrs"]}
    c = collections.Counter()
    for name, cl in classes.items():
        c["classes"] += 1
        c["status:" + cl["status"]] += 1
        c["reactions"] += len(cl.get("on", []))
    for _, a in attrs(classes):
        c["attributes"] += 1
        c["kind:" + a["kind"]] += 1
        c["name:" + a["conf"]] += 1
        o = old.get((phash(a["packet"]), a["i"]))
        if o is None:
            c["schema: attribute was not listed"] += 1
        elif not o.get("name"):
            c["schema: had no name"] += 1
        elif o["name"] != a["name"]:
            c["schema: renamed (%s)" % o.get("name_conf")] += 1
        elif o.get("name_conf") != "exe" and a["conf"] == "exe":
            c["schema: name confirmed (was %s)" % o.get("name_conf")] += 1
        else:
            c["schema: name kept"] += 1
        want = {"msg_in": "in", "msg_out": "out"}.get(a["kind"])
        if want and (o is None or o.get("pin") != want):
            c["schema: message pin new or turned"] += 1
    for k in sorted(c):
        print("%-44s %d" % (k, c[k]))


def kinds():
    classes = load()
    dk = data_kinds()
    c = collections.Counter()
    for _, a in attrs(classes):
        k = dk.get((phash(a["packet"]), a["i"]))
        if k is None:
            c["not stored in the game files"] += 1
        elif k in FITS[a["kind"]]:
            c["agrees (%s as %s)" % (a["kind"], k)] += 1
        else:
            c["DIFFERS"] += 1
            print("differs\t%s %d\tfiles: %s\tcode: %s\t%s" % (a["packet"], a["i"], k, a["kind"], a["name"]))
    for k in sorted(c):
        print("%-44s %d" % (k, c[k]))


def types():
    classes = load()
    by = {(phash(a["packet"]), a["i"]): a for _, a in attrs(classes)}
    t = tomllib.load(open(os.path.join(DATA, "attr_types.toml"), "rb"))["packet"]
    c = collections.Counter()
    for pname, p in sorted(t.items()):
        for e in p["attrs"]:
            a = by.get((int(p["hash"], 16), e["i"]))
            if a is None:
                continue
            c["typed attributes read in code"] += 1
            c["%s (%s)" % (e["type"].split(":")[0], e["conf"])] += 1
            if e["conf"] == "guess" or e["type"] in ("bool", "money"):
                print("%s %d\t%s (%s)\tcode: %s `%s`\t%s" % (
                    pname, e["i"], e["type"], e["conf"], a["kind"], a["name"], a["does"][:110]))
    for k in sorted(c):
        print("%-44s %d" % (k, c[k]))


def corpus(root):
    base = sdbm("Base")
    c = collections.Counter()
    bad = 0
    for path, d, offs, _ in groups(root):
        for o in offs:
            flags, byte_1d = d[o + 0x1C], d[o + 0x1D]
            ng, npk = d[o + 0x1E], d[o + 0x1F]
            value = None
            for k in range(ng, ng + npk):
                q = o + 0x2C + 4 * k
                h, slots = packet(d, q + struct.unpack_from("<i", d, q)[0])
                if h == base:
                    value = struct.unpack_from("<I", d, slots[0])[0] if 0 in slots else 0
            c["objects"] += 1
            c["objects with a Base packet"] += value is not None
            ok = bool(flags & 2) == (value is not None) and byte_1d == (value or 0)
            c["byte 0x1D = %d" % byte_1d] += 1
            if not ok:
                bad += 1
                if bad < 10:
                    print("mismatch %s object at 0x%x: flags %d, byte %d, Base attr 0 %r" % (path, o, flags, byte_1d, value))
    for k in sorted(c):
        print("%-44s %d" % (k, c[k]))
    print("mismatches: %d" % bad)
    return 1 if bad else 0


if __name__ == "__main__":
    cmd = sys.argv[1] if len(sys.argv) > 1 else ""
    if cmd == "stats":
        stats()
    elif cmd == "kinds":
        kinds()
    elif cmd == "types":
        types()
    elif cmd == "tables" and len(sys.argv) > 2:
        tables(sys.argv[2])
    elif cmd == "schema":
        path = os.path.join(DATA, "logic_schema.toml")
        text = apply_schema(open(path).read())
        tomllib.loads(text)
        open(path, "w").write(text)
    elif cmd == "corpus" and len(sys.argv) > 2:
        sys.exit(corpus(sys.argv[2]))
    else:
        sys.exit(__doc__)

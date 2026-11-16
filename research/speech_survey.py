#!/usr/bin/env python3
"""Survey of the speech and mixer resources: `smb`, `get`, `auc`, `mix`.

Prints the value statistics the format documents quote
(docs/formats/dialogues.md §4, §6, docs/formats/audio.md §13, §14): field
values of speech bank entries, their order and name hashes, the fields of the
event table, the categories, pools and ducks of the audio configuration and
the channels of the mixer snapshots.

Usage:
    for t in smb get auc mix; do
        doctor-toolkit dump "$EARS_GF2_DIR" --type $t -o DUMP/$t
    done
    speech_survey.py DUMP
"""
import collections
import glob
import os
import struct
import sys


def sdbm(text):
    h = 0
    for c in text.lower():
        h = (h * 65599 + c) & 0xFFFFFFFF
    return h


def files(root, tag):
    return sorted(glob.glob(os.path.join(root, tag, "**", "*.bin"), recursive=True))


def cstr(data, at, size):
    return data[at:at + size].split(b"\0")[0]


def show(title, counter, n=12):
    print(f"  {title}: {len(counter)} value(s):", counter.most_common(n))


def auc(root):
    names = {}
    paths = files(root, "auc")
    datas = [open(p, "rb").read() for p in paths]
    print(f"auc: {len(paths)} resource(s), identical: {len(set(datas)) == 1}")
    d = datas[0]
    head = struct.unpack_from("<8I", d, 0)
    print("  header:", head)
    cats, pools, ducks = head[4], head[5], head[6]
    at = 0x20
    parents = collections.Counter()
    for _ in range(cats):
        name, second, parent = (cstr(d, at + 32 * k, 32) for k in range(3))
        names[sdbm(name)] = name.decode()
        parents[(parent.decode(), bool(second))] += 1
        at += 100
    show("parents (name, second name set)", parents)
    for _ in range(pools):
        print("  pool", cstr(d, at, 32).decode(), struct.unpack_from("<2I", d, at + 32))
        at += 40
    for _ in range(ducks):
        name = cstr(d, at + 16, 32)
        names[sdbm(name)] = name.decode()
        print("  duck", struct.unpack_from("<2I", d, at), d[at + 8:at + 16], name.decode(),
              cstr(d, at + 48, 128).decode())
        at += 176
    print("  bytes after the ducks:", len(d) - at)
    return names


def smb(root, names):
    stat = collections.defaultdict(collections.Counter)
    total = 0
    paths = files(root, "smb")
    for path in paths:
        d = open(path, "rb").read()
        head = struct.unpack_from("<6I", d, 0)
        stat["header words 0..3"][head[:4]] += 1
        n, table = head[4], head[5]
        offsets = struct.unpack_from(f"<{n}I", d, table)
        ids = []
        expected = table + 4 * n
        for o in offsets:
            stat["gap before the entry"][o - expected] += 1
            expected = o + 0x70
            e = d[o:o + 0x70]
            tag, flags = struct.unpack_from("<HH", e, 0)
            guid = struct.unpack_from("<4I", e, 4)
            ids.append(guid)
            stat["tag"][tag] += 1
            stat["flags"][hex(flags)] += 1
            stat["distinct id words"][len(set(guid))] += 1
            stat["word 0x14"][struct.unpack_from("<I", e, 0x14)[0]] += 1
            for k in range(6):
                stat[f"f32 {0x18 + 4 * k:#x}"][round(struct.unpack_from("<f", e, 0x18 + 4 * k)[0], 3)] += 1
            stat["u32 0x30"][struct.unpack_from("<I", e, 0x30)[0]] += 1
            for k, what in ((0x34, "category"), (0x38, "category 2"), (0x3C, "duck")):
                value = struct.unpack_from("<I", e, k)[0]
                stat[what][names.get(value, f"{value:08x}")] += 1
            name = cstr(e, 0x40, 0x28)
            extra, name_hash = struct.unpack_from("<II", e, 0x68)
            stat["extra offset 0x68"][extra] += 1
            stem = name.rsplit(b".", 1)[0]
            stat["hash 0x6C == sdbm(stem + '.stx')"][name_hash == sdbm(stem + b".stx")] += 1
            total += 1
        stat["entries sorted by id"][ids == sorted(ids)] += 1
        stat["bytes after the last entry"][len(d) - expected] += 1
    print(f"smb: {len(paths)} resource(s), {total} entr(ies)")
    for key, counter in stat.items():
        show(key, counter)


def get(root):
    paths = files(root, "get")
    d = open(paths[0], "rb").read()
    version, n, at = struct.unpack_from("<HHI", d, 0)
    print(f"get: {len(paths)} resource(s), version {version}, {n} record(s), "
          f"size {len(d)} = 8 + 36 * {n}: {len(d) == 8 + 36 * n}")
    stat = collections.defaultdict(collections.Counter)
    keys = []
    for i in range(n):
        r = d[at + 36 * i:at + 36 * i + 36]
        w = struct.unpack("<9I", r)
        f = struct.unpack("<9f", r)
        keys.append(w[0])
        stat["f32 0x04"][f[1]] += 1
        stat["f32 0x08 cool-down"][f[2]] += 1
        stat["f32 0x0C"][f[3]] += 1
        stat["u32 0x10"][w[4]] += 1
        stat["f32 0x14"][f[5]] += 1
        stat["i32 0x18 rank"][w[6]] += 1
        stat["u32 0x1C flags"][hex(w[7])] += 1
        stat["u32 0x20 priority"][w[8]] += 1
        stat["u8 0x20"][r[32]] += 1
    print("  sorted by hash, unique:", keys == sorted(keys), len(set(keys)) == n)
    for key, counter in stat.items():
        show(key, counter, 24)


def mix(root, names):
    stat = collections.defaultdict(collections.Counter)
    paths = files(root, "mix")
    channels = 0
    for path in paths:
        d = open(path, "rb").read()
        stat["words 0..6"][struct.unpack_from("<7I", d, 0)] += 1
        body = 0x1C
        name = cstr(d, body, 32)
        version, flags, n, first, n2, end = struct.unpack_from("<6I", d, body + 0x20)
        stat["version, flags, first, second count"][(version, flags, first, n2)] += 1
        stat["end == 0x38 + 44 * n"][end == 0x38 + 44 * n] += 1
        stat["size == 0x1C + end"][len(d) == body + end] += 1
        stat["name length"][len(name)] += 1
        at = body + first
        for i in range(n):
            w = struct.unpack_from("<11I", d, at)
            f = struct.unpack_from("<11f", d, at)
            stat["category in auc"][w[0] in names] += 1
            for k in range(1, 9):
                stat[f"level {k - 1}"][round(f[k], 3)] += 1
            stat["levels 6 and 7 equal"][w[7] == w[8]] += 1
            stat["frequency"][w[9]] += 1
            stat["next offset"][(w[10], i == n - 1)] += 1
            at += 44
            channels += 1
    print(f"mix: {len(paths)} resource(s), {channels} channel(s)")
    for key, counter in stat.items():
        show(key, counter, 14)


def main():
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    root = sys.argv[1]
    names = auc(root)
    smb(root, names)
    get(root)
    mix(root, names)


if __name__ == "__main__":
    main()

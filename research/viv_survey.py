#!/usr/bin/env python3
"""Survey of the BIGF archives (`GODFATH*.viv`) in the game directory.

Prints entry counts and sizes per directory and extension, and the first bytes
of an example entry of every extension.

Usage: viv_survey.py <game_dir> [--list]
"""
import collections
import os
import struct
import sys


def entries(path):
    with open(path, "rb") as f:
        head = f.read(16)
        magic, total, count, header = struct.unpack(">4sIII", head)
        total = struct.unpack_from("<I", head, 4)[0]
        assert magic == b"BIGF", magic
        table = f.read(header - 16)
    pos = 0
    for _ in range(count):
        offset, size = struct.unpack_from(">II", table, pos)
        end = table.index(b"\0", pos + 8)
        yield offset, size, table[pos + 8:end].decode("latin1")
        pos = end + 1


def main(root, show_list):
    for fn in sorted(os.listdir(root)):
        if not fn.lower().endswith(".viv"):
            continue
        path = os.path.join(root, fn)
        by_ext = collections.Counter()
        size_ext = collections.Counter()
        by_dir = collections.Counter()
        example = {}
        n = 0
        for offset, size, name in entries(path):
            n += 1
            ext = name.lower().split("\\")[-1].split(".", 1)[-1]
            by_ext[ext] += 1
            size_ext[ext] += size
            by_dir[name.lower().split("\\")[0]] += 1
            example.setdefault(ext, (offset, name))
            if show_list:
                print(f"{fn} {offset:#012x} {size:10} {name}")
        print(f"{fn}: {n} entries, {os.path.getsize(path)} bytes")
        for d, c in by_dir.most_common():
            print(f"   dir {d:30} {c}")
        with open(path, "rb") as f:
            for ext, c in by_ext.most_common():
                offset, name = example[ext]
                f.seek(offset)
                head = f.read(16)
                print(f"   .{ext:20} {c:6} {size_ext[ext]:12}  {head.hex()}  {name}")


if __name__ == "__main__":
    main(sys.argv[1], "--list" in sys.argv[2:])

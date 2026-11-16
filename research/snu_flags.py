#!/usr/bin/env python3
"""Byte 1 of the SNU header against the metadata of the file.

Reads every seventh entry of the `GODFATH*.viv` archives and counts the
combinations of the header bytes 0..3, the ids of the metadata chunks and
whether the field at 0x0C is set. Shows that byte 1 (0, 4, 8 or 12) does not
tell whether a speech file has metadata (docs/formats/audio.md §4).

Usage: snu_flags.py <game_dir> [step]
"""
import collections
import glob
import os
import struct
import sys


def entries(handle):
    head = handle.read(16)
    count, header = struct.unpack_from(">II", head, 8)
    table = handle.read(header - 16)
    pos = 0
    for _ in range(count):
        offset, size = struct.unpack_from(">II", table, pos)
        end = table.index(b"\0", pos + 8)
        yield offset, size, table[pos + 8:end].decode("latin1")
        pos = end + 1


def main(root, step):
    counts = collections.Counter()
    example = {}
    for path in sorted(glob.glob(os.path.join(root, "GODFATH*.viv"))):
        with open(path, "rb") as handle:
            for i, (offset, _size, name) in enumerate(list(entries(handle))):
                if i % step:
                    continue
                handle.seek(offset)
                d = handle.read(0x60)
                if len(d) < 0x40:
                    continue
                start = struct.unpack_from("<I", d, 8)[0]
                ids = ()
                if start > 0x20 and struct.unpack_from("<I", d, 0x20)[0] == 0xCAE87732:
                    n = struct.unpack_from("<I", d, 0x2C)[0]
                    ids = tuple(f"{struct.unpack_from('<I', d, 0x30 + 12 * j)[0]:08x}"
                                for j in range(min(n, 4)))
                key = (d[0], d[1], d[3], ids, struct.unpack_from("<I", d, 0x0C)[0] != 0)
                counts[key] += 1
                example.setdefault(key, name)
    print("files  byte0 byte1 channels  chunks  field 0x0C set  example")
    for key, n in counts.most_common():
        print(n, *key, example[key])


if __name__ == "__main__":
    if len(sys.argv) < 2:
        sys.exit(__doc__)
    main(sys.argv[1], int(sys.argv[2]) if len(sys.argv) > 2 else 7)

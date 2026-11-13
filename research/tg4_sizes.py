#!/usr/bin/env python3
"""Checks that tg4d data sizes match the mip chain implied by the tg4h header.

Usage: tg4_sizes.py <tg4h_dump_dir>
"""
import collections
import os
import struct
import sys

BLOCK = {"DXT1c": (4, 8), "DXT1a": (4, 8), "DXT5": (4, 16), "DXT5_NM": (4, 16),
         "L8": (1, 1), "A8": (1, 1), "L8A8": (1, 2), "RGBA8": (1, 4)}
FORMAT, DATASIZE, KIND = 0xDEBA72DF, 0x4F8B2A9F, 0x97CA47AB


def cstr(d, o):
    return d[o:d.index(b"\0", o)].decode()


def level_size(fmt, w, h):
    b, n = BLOCK[fmt]
    return max(1, (w + b - 1) // b) * max(1, (h + b - 1) // b) * n


def chain(fmt, w, h, mips, faces=1, depth=1):
    total = 0
    for m in range(mips):
        total += level_size(fmt, max(1, w >> m), max(1, h >> m)) * max(1, depth >> m)
    return total * faces


def main(root):
    c = collections.Counter()
    odd = []
    for dp, _, fns in os.walk(root):
        for fn in fns:
            d = open(os.path.join(dp, fn), "rb").read()
            name_hash, name_off, size, w, h, u24, mips, n, names, vals = struct.unpack_from("<IIIHHHBBII", d, 0x14)
            p = {}
            for i in range(n):
                ph = struct.unpack_from("<I", d, names + 4 * i)[0]
                t, v = struct.unpack_from("<BI", d, vals + 5 * i)
                p[ph] = cstr(d, v) if t == 0 else v
            fmt, kind = p[FORMAT], p.get(KIND, "2d")
            c["kind:" + kind] += 1
            if kind in ("cube", "3d"):
                # The count byte holds faces (cube) or slices (3d); there are no mip levels.
                ok = size == level_size(fmt, w, h) * mips
                c[f"{kind} count {mips}"] += 1
            else:
                ok = size == chain(fmt, w, h, mips)
            c[f"{kind}:{'ok' if ok else 'mismatch'}"] += 1
            c["datasize_param_eq"] += p[DATASIZE] == size
            c["pow2"] += (w & (w - 1)) == 0 and (h & (h - 1)) == 0
            if not ok and len(odd) < 8:
                odd.append((fn, fmt, kind, w, h, mips, size, chain(fmt, w, h, mips)))
    for k in sorted(c):
        print(f"{k:24} {c[k]}")
    print(odd)


if __name__ == "__main__":
    main(sys.argv[1])

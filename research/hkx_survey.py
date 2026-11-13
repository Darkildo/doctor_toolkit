#!/usr/bin/env python3
"""Survey of Havok packfiles (`hkx` resources) dumped with `doctor-toolkit dump --type hkx`.

Prints header variants, the contents (root) class and the classes of all
objects found through virtual fixups.

Usage: hkx_survey.py <dump_dir> [class_to_show]
"""
import collections
import os
import struct
import sys


def sections(d):
    n = struct.unpack_from("<i", d, 0x14)[0]
    out = []
    for i in range(n):
        at = 0x40 + 0x30 * i
        tag = d[at:at + 19].split(b"\0")[0].decode()
        start, loc, glob, virt, exp, imp, end = struct.unpack_from("<7I", d, at + 20)
        out.append((tag, start, loc, glob, virt, exp, imp, end))
    return out


def virtual_fixups(d, secs):
    out = []
    for tag, start, loc, glob, virt, exp, imp, end in secs:
        pos = start + virt
        while pos + 12 <= start + exp:
            src, sec, off = struct.unpack_from("<3I", d, pos)
            if src == 0xFFFFFFFF:
                break
            cs = secs[sec][1]
            name = d[cs + off:d.index(b"\0", cs + off)].decode()
            out.append((tag, src, name))
            pos += 12
    return out


def main(root, show):
    c = collections.Counter()
    roots = collections.Counter()
    classes = collections.Counter()
    combos = collections.Counter()
    example = {}
    for dp, _, fns in os.walk(root):
        for fn in fns:
            p = os.path.join(dp, fn)
            d = open(p, "rb").read()
            m0, m1, tag, ver = struct.unpack_from("<IIii", d, 0)
            c[f"magic={m0:08x}/{m1:08x} ver={ver} layout={d[16:20].hex()}"] += 1
            c["version=" + d[0x28:0x38].split(b"\0")[0].decode()] += 1
            secs = sections(d)
            c["sections=" + ",".join(s[0] for s in secs)] += 1
            idx, off, cidx, coff = struct.unpack_from("<4i", d, 0x18)
            cs = secs[cidx][1]
            rootname = d[cs + coff:d.index(b"\0", cs + coff)].decode()
            roots[rootname] += 1
            vf = virtual_fixups(d, secs)
            names = collections.Counter(n for _, _, n in vf)
            for n in names:
                classes[n] += 1
                example.setdefault(n, p)
            combos[" ".join(sorted(names))] += 1
            c["end==len"] += secs[-1][1] + secs[-1][7] == len(d)
    for k, v in sorted(c.items()):
        print(f"{v:6} {k}")
    print("\nroot classes:")
    for k, v in roots.most_common():
        print(f"{v:6} {k}")
    print("\nfiles containing class:")
    for k, v in classes.most_common():
        print(f"{v:6} {k}")
    print("\nclass combinations:")
    for k, v in combos.most_common(25):
        print(f"{v:6} {k}")
    if show:
        print("\nexample:", example.get(show))


if __name__ == "__main__":
    main(sys.argv[1], sys.argv[2] if len(sys.argv) > 2 else None)

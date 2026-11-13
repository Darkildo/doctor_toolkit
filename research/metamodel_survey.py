#!/usr/bin/env python3
"""Checks the metamodel (MMdl v14) layout on resources dumped with `doctor-toolkit dump --type metamodel`.

Usage: metamodel_survey.py <dump_dir>

The resource is a memory image: pointers are absolute offsets from its start.

    0x00  "ldMM" (u32 0x4D4D646C), u32 version 14, u32 size, u32 0
    0x10  4 x u32 resource id, u32 hash, u32 id, u32 hash, u32 hash, u32 LOD/flags
    0x34  9 x u32 table pointers, 0x58 8 x u16 table counts (the last pointer has no count)
    0x68  tables, data blob (table 7), strings

Tables (stride): 0 resource references (0x20), 1 states (0x30), 2 variables (0x0C),
3 properties (0x18), 4 conditions (0x0C), 5 parts (0x24), 6 sub-objects (0x24).
"""
import collections
import os
import struct
import sys

STRIDE = [0x20, 0x30, 0x0C, 0x18, 0x0C, 0x24, 0x24]


def main(root):
    c = collections.Counter()
    for dp, _, fns in os.walk(root):
        for fn in fns:
            d = open(os.path.join(dp, fn), "rb").read()
            n = len(d)
            magic, ver, size, z = struct.unpack_from("<4I", d, 0)
            c["files"] += 1
            c["header_ok"] += (magic, ver, size, z) == (0x4D4D646C, 14, n, 0)
            P = struct.unpack_from("<9I", d, 0x34)
            C = struct.unpack_from("<8H", d, 0x58)
            c["p8_c7_zero"] += P[8] == 0 and C[7] == 0
            spans = [(P[i], P[i] + C[i] * STRIDE[i]) for i in range(7)]

            def table_of(v):
                for i, (a, b) in enumerate(spans):
                    if a <= v < b and (v - a) % STRIDE[i] == 0:
                        return i
                if P[7] and P[7] <= v < n:
                    return "blob"
                return None

            def strp(v, what):
                ok = 0x68 <= v < n and v > 0 and d[v - 1] == 0 and b"\0" in d[v:]
                c["string_ok:" + what] += ok

            # Tables follow each other in order, starting at 0x68.
            pos = 0x68
            ordered = True
            for i in range(7):
                if C[i]:
                    ordered &= P[i] >= pos
                    pos = P[i] + C[i] * STRIDE[i]
            c["tables_in_order"] += ordered
            lists = []
            for k in range(C[1]):
                e = P[1] + 0x30 * k
                h, tn, nm = struct.unpack_from("<3I", d, e)
                strp(tn, "state_type")
                strp(nm, "state_name")
                pa, ca, cb, pb, pc, cb2, cc = struct.unpack_from("<IHHIIHH", d, e + 0x1C)
                c["state_cb:%d" % cb] += 1
                lists += [("state_a", pa, ca), ("state_b", pb, cb2), ("state_c", pc, cc)]
            for k in range(C[2]):
                nm, h, kind = struct.unpack_from("<3I", d, P[2] + 0xC * k)
                strp(nm, "variable")
                c["variable_kind:%d" % kind] += 1
            for k in range(C[3]):
                val, nm, cls, idx, h, ty = struct.unpack_from("<6I", d, P[3] + 0x18 * k)
                strp(nm, "property_name")
                strp(cls, "property_class")
                if ty in (0x08, 0x10, 0x20, 0x80, 0x100, 0x200) and val:
                    c["property_type_%x->%s" % (ty, table_of(val) if ty != 8 else "string")] += 1
                    if ty == 8:
                        strp(val, "property_value")
                else:
                    c["property_type_%x" % ty] += 1
            for k in range(C[4]):
                a, p, op = struct.unpack_from("<3I", d, P[4] + 0xC * k)
                c["condition_op:%d->%s" % (op, table_of(p))] += 1
            for i in (5, 6):
                for k in range(C[i]):
                    e = P[i] + 0x24 * k
                    h, tn, nm = struct.unpack_from("<3I", d, e)
                    strp(tn, "t%d_type" % i)
                    strp(nm, "t%d_name" % i)
                    pa, ca, cb = struct.unpack_from("<IHH", d, e + 0x1C)
                    c["t%d_cb:%d" % (i, cb)] += 1
                    lists.append(("t%d" % i, pa, ca))
            for what, p, count in lists:
                if not count:
                    c["list_empty_ptr_zero:" + what] += p == 0
                    continue
                for v in struct.unpack_from("<%dI" % count, d, p):
                    c["list:%s->%s" % (what, table_of(v))] += 1
    for k in sorted(c):
        print(f"{k:40} {c[k]}")


if __name__ == "__main__":
    main(sys.argv[1])

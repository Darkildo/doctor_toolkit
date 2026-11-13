#!/usr/bin/env python3
"""Survey of tg4h texture headers dumped with `doctor-toolkit dump --type tg4h`.

Checks the layout hypothesis and collects formats, parameter names and value types.

Usage: tg4_survey.py <dump_dir>
"""
import collections
import os
import struct
import sys


def sdbm(s):
    h = 0
    for ch in s:
        h = (ch + h * 65599) & 0xFFFFFFFF
    return h


def cstr(d, o):
    return d[o:d.index(b"\0", o)]


def main(root):
    c = collections.Counter()
    fields = collections.defaultdict(collections.Counter)
    params = collections.defaultdict(collections.Counter)
    fmt = collections.Counter()
    bad = []
    for dp, _, fns in os.walk(root):
        for fn in fns:
            d = open(os.path.join(dp, fn), "rb").read()
            c["files"] += 1
            pkg_off, ver, ntex, a08, rec_off, a10 = struct.unpack_from("<IHHIII", d, 0)
            fields["ver"][ver] += 1
            fields["ntex"][ntex] += 1
            fields["a08"][a08] += 1
            fields["rec_off"][rec_off] += 1
            fields["a10"][a10] += 1
            for t in range(ntex):
                r = rec_off - 4 + t * 0x24  # hypothesis: records start at 0x14
                r = 0x14 + t * 0x24
                name_hash, name_off, data_size, w, h, depth, mips, nparams, names_off, vals_off = struct.unpack_from("<IIIHHHBBII", d, r)
                name = cstr(d, name_off)
                fields["depth"][depth] += 1
                c["name_hash==sdbm(name)"] += sdbm(name) == name_hash
                c["name_hash==sdbm(lower)"] += sdbm(name.lower()) == name_hash
                pv = {}
                for i in range(nparams):
                    ph = struct.unpack_from("<I", d, names_off + 4 * i)[0]
                    ptype, pval = struct.unpack_from("<BI", d, vals_off + 5 * i)
                    v = cstr(d, pval).decode() if ptype == 0 else pval
                    pv[ph] = v
                    params[ph][(ptype, v if ptype == 0 else "int")] += 1
                f = next((v for v in pv.values() if isinstance(v, str) and v.upper().startswith(("DXT", "ARGB", "RGB", "L8", "A8", "ATI", "BC", "R", "X"))), None)
                fmt[f] += 1
                fields["mips"][mips] += 1
                fields["nparams"][nparams] += 1
                if len(bad) < 5 and data_size not in pv.values():
                    bad.append((fn, data_size, pv))
    for k in sorted(c):
        print(f"{k:28} {c[k]}")
    for k, v in fields.items():
        print(k, v.most_common(8))
    print("formats", fmt.most_common(20))
    for h, vals in params.items():
        print(f"param {h:08x}", vals.most_common(6))
    print("data size not in params:", bad)


if __name__ == "__main__":
    main(sys.argv[1])

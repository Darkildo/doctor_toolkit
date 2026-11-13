#!/usr/bin/env python3
"""Corpus survey of StreamTOC v10: layout hypotheses, size field, name hash.

Usage: toc_layout.py <game_dir>
"""
import collections
import glob
import os
import struct
import sys


def sdbm(s, h=0):
    for ch in s:
        h = (ch + h * 65599) & 0xFFFFFFFF
    return h


def stream_stats(path):
    d = open(path, "rb").read()
    pos = 12
    tot = n = blocks_bytes = 0
    tot_al = 0
    last = 12
    while pos + 8 <= len(d):
        tag, size = struct.unpack_from("<4sI", d, pos)
        if tag == b"COHS":
            if d[pos + 8:pos + 12] == b"RDHS":
                al = struct.unpack_from("<I", d, pos + 16)[0]
                ds = struct.unpack_from("<I", d, pos + 40)[0]
                tot += ds
                tot_al += (ds + al - 1) // al * al
                n += 1
            blocks_bytes += size
            last = pos + size
        elif tag != b"LLIF":
            break
        pos += size
    return dict(file=len(d), data=tot, data_al=tot_al, n=n, blocks=blocks_bytes, last=last)


def find_ci(root, rel):
    cur = root
    for part in rel.split("/"):
        try:
            m = [e for e in os.listdir(cur) if e.lower() == part.lower()]
        except OSError:
            return None
        if not m:
            return None
        cur = os.path.join(cur, m[0])
    return cur


def main(root):
    c = collections.Counter()
    size_hyp = collections.Counter()
    hash_hyp = collections.Counter()
    flags = collections.Counter()
    extra = []
    diffs = []
    tocs = sorted(glob.glob(os.path.join(root, "**", "*.toc"), recursive=True))
    for p in tocs:
        d = open(p, "rb").read()
        nf, nn, cc, cd = struct.unpack_from("<4H", d, 8)
        ents = [struct.unpack_from("<II4BII", d, 0x10 + 20 * i) for i in range(nf)]
        nt = 0x10 + 20 * nf
        names = [struct.unpack_from("<II", d, nt + 8 * i) for i in range(nn)]
        dep0 = nt + 8 * nn
        str0 = min(o for _, o in names)
        c["tocs"] += 1
        c["names_sorted"] += [h for h, _ in names] == sorted(h for h, _ in names)
        c["cc_eq_depwords"] += cc == (str0 - 1 - dep0) // 4
        c["dep_region_mod4"] += (str0 - 1 - dep0) % 4 == 0
        c["byte_before_strings_zero"] += d[str0 - 1] == 0
        used = len(d.rstrip(b"\0"))
        c["len_mod_0x800"] += len(d) % 0x800 == 0
        twin = p.replace("_global.toc", ".toc") if p.endswith("_global.toc") else p.replace(".toc", "_global.toc")
        if os.path.exists(twin):
            c["twin_identical"] += open(twin, "rb").read() == d
        name_of = {}
        for h, o in names:
            name_of[nt + 8 * names.index((h, o))] = (h, d[o:d.index(b"\0", o)].decode("latin1"))
        ref = set()
        for e in ents:
            rec, size, fa, fb, fc, fd, z, dep = e
            ref.add(rec)
            h, nm = name_of.get(rec, (None, None))
            if nm is None:
                c["bad_name_ptr"] += 1
                continue
            flags[(fa, fb, fc, fd)] += 1
            c["z_zero"] += z == 0
            ndeps = fa + fb + (fc & 1)
            deps = struct.unpack_from("<%dI" % ndeps, d, dep) if ndeps else ()
            c["deps_point_to_names"] += all(x in name_of for x in deps)
            c["dep_in_region"] += dep0 <= dep <= str0
            f = find_ci(root, nm.replace("\\", "/") + ".str")
            c["entries"] += 1
            if f is None:
                c["file_missing"] += 1
                continue
            if os.sep + "_toolkit" + os.sep in f:
                continue
            st = stream_stats(f)
            hit = [k for k, v in st.items() if v == size]
            size_hyp[tuple(hit) or ("none",)] += 1
            if not hit and len(diffs) < 6:
                diffs.append((nm, size, st))
            base = nm.encode()
            cands = {
                "name": base, "lower": base.lower(), "upper": base.upper(),
                "fwd": base.replace(b"\\", b"/"), "str": base + b".str",
                "tail": base.split(b"\\")[-1], "notop": base.split(b"\\", 1)[-1],
            }
            ok = [k for k, v in cands.items() if sdbm(v) == h]
            hash_hyp[tuple(ok) or ("none",)] += 1
        for i, (h, o) in enumerate(names):
            if nt + 8 * i not in ref:
                raw = d[o:d.index(b"\0", o)]
                for k, v in (("x_name", raw), ("x_lower", raw.lower()), ("x_upper", raw.upper()), ("x_fwd", raw.replace(b"\\", b"/"))):
                    if h and sdbm(v) == h:
                        hash_hyp[("extra", k)] += 1
                extra.append((os.path.basename(p), "%08x" % h, d[o:d.index(b"\0", o)].decode("latin1")))
    for k in sorted(c):
        print(f"{k:26} {c[k]}")
    print("flags", flags.most_common(10))
    print("size field ==", size_hyp.most_common(8))
    print("size mismatches", diffs[:4])
    print("hash ==", hash_hyp.most_common(8))
    print("unreferenced names", len(extra), extra[:8])


if __name__ == "__main__":
    main(sys.argv[1])

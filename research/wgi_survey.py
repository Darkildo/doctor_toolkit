#!/usr/bin/env python3
"""Corpus survey of the world grid resources (`wgi`).

Usage: wgi_survey.py <game_dir> <wgi_dump_dir>

<wgi_dump_dir> is the output of `doctor-toolkit dump --type wgi -o DIR GAME`.
Prints the layout checks behind docs/formats/wgi.md: section order, pointer
targets, which hashes are stream hashes of the tables of contents, cell kinds,
flags, name hashes and the ring links.
"""
import collections
import glob
import os
import struct
import sys

MAGIC = 0x3B004EB9
NONE = 0xFE16702F


def sdbm(s, h=0):
    for ch in s:
        h = (ch + h * 65599) & 0xFFFFFFFF
    return h


def fold(g):
    a, b, c, d = struct.unpack("<4I", g)
    return (((a * 33 + b) * 33 + c) * 33 + d) & 0xFFFFFFFF


def load_tocs(root):
    """hash -> (name, data_size, flags, [dependency names])"""
    out = {}
    for p in sorted(glob.glob(os.path.join(root, "**", "*.toc"), recursive=True)):
        if os.sep + "_toolkit" + os.sep in p or os.sep + "_doctor_toolkit" + os.sep in p:
            continue
        d = open(p, "rb").read()
        nf, nn = struct.unpack_from("<2H", d, 8)
        nt = 0x10 + 20 * nf
        name_at = {}
        for i in range(nn):
            h, o = struct.unpack_from("<II", d, nt + 8 * i)
            name_at[nt + 8 * i] = (h, d[o:d.index(b"\0", o)].decode("latin1"))
        for i in range(nf):
            rec, size, fa, fb, fc, fd, z, dep = struct.unpack_from("<II4BII", d, 0x10 + 20 * i)
            h, nm = name_at[rec]
            n = fa + fb + (fc & 1)
            deps = [name_at[x][1] for x in struct.unpack_from("<%dI" % n, d, dep)] if n else []
            out.setdefault(h, (nm, size, (fa, fb, fc, fd), deps))
    return out


def main(root, dump):
    toc = load_tocs(root)
    c = collections.Counter()
    kinds = collections.Counter()
    flags51 = collections.Counter()
    kind_flags = collections.Counter()
    w52 = collections.Counter()
    w54 = collections.Counter()
    f58 = collections.Counter()
    f5c = collections.Counter()
    tails = collections.Counter()
    ring = collections.Counter()
    ring_kinds = collections.Counter()
    named = []
    kind_names = collections.defaultdict(list)
    files = sorted(glob.glob(os.path.join(dump, "**", "*_wgi_*.bin"), recursive=True))
    for p in files:
        d = open(p, "rb").read()
        magic, ver, so, ns, co, nc, po, npool = struct.unpack_from("<8I", d, 0)
        c["files"] += 1
        c["magic_version_ok"] += magic == MAGIC and ver == 7
        c["streams_at_0x20"] += so == 0x20
        c["cells_follow_streams"] += co == so + 24 * ns
        c["pool_follows_cells"] += po == co + 0x80 * nc
        c["file_ends_after_pool"] += len(d) == po + 4 * npool
        pool = struct.unpack_from("<%dI" % npool, d, po)
        c["pool_points_to_streams"] += all(
            so <= x < co and (x - so) % 24 == 0 for x in pool)
        pool_used = set()
        for i in range(ns):
            o = so + 24 * i
            h = fold(d[o:o + 16])
            n, off = struct.unpack_from("<II", d, o + 16)
            c["streams"] += 1
            c["stream_in_toc"] += h in toc
            c["stream_list_empty"] += n == 0
            if n == 0:
                c["stream_empty_list_ptr_zero"] += off == 0
                continue
            c["stream_list_in_pool"] += po <= off and off + 4 * n <= po + 4 * npool
            idx = (off - po) // 4
            pool_used.update(range(idx, idx + n))
            if h in toc:
                names = []
                for x in pool[idx:idx + n]:
                    hh = fold(d[x:x + 16])
                    names.append(toc[hh][0] if hh in toc else "?")
                nm, _, fl, deps = toc[h]
                c["stream_list_checked"] += 1
                c["stream_list_eq_toc_prereqs"] += names == deps[:fl[0] + (fl[2] & 1)]
                c["stream_list_eq_toc_all_deps"] += names == deps
                c["stream_list_sets_overlap_toc_deps"] += bool(set(names) & set(deps))
                if names != deps and c["stream_list_examples"] < 6:
                    c["stream_list_examples"] += 1
                    print("list example:", nm, fl, "wgi:", names, "toc:", deps)
        for i in range(nc):
            o = co + 0x80 * i
            b = d[o:o + 0x80]
            c["cells"] += 1
            sh, = struct.unpack_from("<I", b, 0x10)
            in_toc = sh in toc
            c["cell_hash_in_toc"] += in_toc
            c["cell_hash_eq_fold_guid"] += sh == fold(b[:16])
            cx, cy, cz, rad = struct.unpack_from("<4f", b, 0x14)
            c["cell_0x24_zero"] += b[0x24:0x30] == bytes(12)
            mn = struct.unpack_from("<4f", b, 0x30)
            mx = struct.unpack_from("<4f", b, 0x40)
            empty_box = mn[0] > mx[0]
            c["cell_box_empty"] += empty_box
            c["cell_box_w_one"] += mn[3] == 1.0 and mx[3] == 1.0
            if not empty_box:
                ctr = [(mn[k] + mx[k]) / 2 for k in range(3)]
                c["cell_centre_is_box_centre"] += all(
                    abs(ctr[k] - (cx, cy, cz)[k]) < 0.01 for k in range(3))
                half = sum(((mx[k] - mn[k]) / 2) ** 2 for k in range(3)) ** 0.5
                c["cell_radius_is_box_half_diagonal"] += abs(half - rad) < 0.01 * max(1.0, rad)
            kind, fl = b[0x50], b[0x51]
            kinds[kind] += 1
            flags51[fl] += 1
            kind_flags[(kind, fl)] += 1
            w52[struct.unpack_from("<H", b, 0x52)[0]] += 1
            w54[struct.unpack_from("<I", b, 0x54)[0]] += 1
            f58[struct.unpack_from("<f", b, 0x58)[0]] += 1
            f5c[struct.unpack_from("<f", b, 0x5C)[0]] += 1
            nh, w64, w68, plist, link = struct.unpack_from("<5I", b, 0x60)
            tails[b[0x74:0x80].hex()] += 1
            c["cell_0x64_zero"] += w64 == 0
            c["cell_0x68_none"] += w68 == NONE
            c["cell_link_is_cell"] += co <= link < po and (link - co) % 0x80 == 0
            c["cell_link_self"] += link == o
            n, x, ks = 1, link, [kind]
            while x != o and n < 1000:
                ks.append(d[x + 0x50])
                x = struct.unpack_from("<I", d, x + 0x70)[0]
                n += 1
            ring[n] += 1
            if n > 1:
                ring_kinds[tuple(sorted(ks))] += 1
            c["cell_list_null"] += plist == 0
            if plist:
                c["cell_list_in_pool"] += po <= plist < po + 4 * npool
                pool_used.add((plist - po) // 4)
            name = toc[sh][0] if in_toc else None
            if name and len(kind_names[kind]) < 4:
                kind_names[kind].append(name)
            if nh:
                c["cell_named"] += 1
                last = name.split("\\")[-1].lower() if name else ""
                ok = sdbm(last.encode()) == nh
                c["cell_name_hash_is_last_component"] += ok
                named.append((os.path.relpath(p, dump), "%08x" % nh, name, kind, fl, ok))
            if name and plist:
                x = pool[(plist - po) // 4]
                hh = fold(d[x:x + 16])
                c["cell_list_first_is_own_stream"] += hh == sh
                nm, _, fl2, deps = toc[sh]
                c["cell_list_first_is_parent"] += (
                    bool(deps) and hh in toc and toc[hh][0] == deps[0])
        c["pool_words"] += npool
        c["pool_words_referenced"] += len(pool_used)
    for k in sorted(c):
        print("%-36s %d" % (k, c[k]))
    print("kinds (+0x50):", dict(kinds))
    for k in sorted(kind_names):
        print("  kind %d e.g. %s" % (k, ", ".join(kind_names[k])))
    print("flags (+0x51):", {hex(k): v for k, v in flags51.items()})
    print("kind, flags:", {"%d/%#x" % k: v for k, v in sorted(kind_flags.items())})
    print("u16 +0x52:", dict(w52))
    print("u32 +0x54:", {hex(k): v for k, v in w54.items()})
    print("f32 +0x58:", dict(f58))
    print("f32 +0x5c:", dict(f5c))
    print("bytes +0x74:", dict(tails))
    print("ring length:", dict(sorted(ring.items())))
    print("ring kinds:", {str(k): v for k, v in ring_kinds.items()})
    for row in named:
        print("named", *row)


if __name__ == "__main__":
    main(sys.argv[1], sys.argv[2])

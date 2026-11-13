#!/usr/bin/env python3
"""Corpus survey of the UStream block layout (pages, FILL blocks, SHDR fields).

Usage: ustream_layout.py <game_dir>
"""
import collections
import os
import struct
import sys

PAGE = 0x20000


def walk(d):
    """Yield (pos, tag, size, subtype) for every block, including FILL."""
    pos = struct.unpack_from("<I", d, 4)[0]
    while pos + 8 <= len(d):
        tag, size = struct.unpack_from("<4sI", d, pos)
        if tag == b"COHS":
            yield pos, tag, size, d[pos + 8:pos + 12]
        elif tag == b"LLIF":
            yield pos, tag, size, b""
        else:
            yield pos, tag, None, b""
            return
        if size < 8:
            return
        pos += size


def main(root):
    c = collections.Counter()
    fill_sizes = collections.Counter()
    fill_body = collections.Counter()
    type_stats = collections.defaultdict(lambda: [0, 0, collections.Counter(), collections.Counter(), collections.Counter()])
    idpat = collections.Counter()
    examples = {}
    rpak_unc = collections.Counter()
    for dp, _, fns in os.walk(root):
        for fn in fns:
            if not fn.lower().endswith(".str"):
                continue
            p = os.path.join(dp, fn)
            d = open(p, "rb").read()
            c["files"] += 1
            c["size_multiple_of_page"] += len(d) % PAGE == 0
            end = None
            cur = None
            for pos, tag, size, st in walk(d):
                if size is None:
                    c["unknown_tag:" + tag.hex()] += 1
                    examples.setdefault("unknown", (p, pos))
                    end = pos
                    break
                end = pos + size
                crosses = pos // PAGE != (pos + size - 1) // PAGE
                if tag == b"LLIF":
                    c["fill"] += 1
                    c["fill_ends_on_page"] += (pos + size) % PAGE == 0
                    c["fill_is_last"] += pos + size == len(d)
                    fill_sizes[min(size, 64) if size < 64 else "big"] += 1
                    body = d[pos + 8:pos + size]
                    fill_body["zero" if not any(body) else "nonzero"] += 1
                    continue
                c["blocks"] += 1
                c["block_crosses_page"] += crosses
                c["size_mod4:%d" % (size % 4)] += 1
                if st == b"RDHS":
                    f0, align, tid, a, b_, c_, d_, dsz = struct.unpack_from("<8I", d, pos + 12)
                    cur = [tid, dsz, 0, pos, align]
                    ts = type_stats[tid]
                    ts[0] += 1
                    ts[1] += dsz
                    ts[2][f0] += 1
                    ts[3][align] += 1
                    pat = (a == b_, b_ == c_, c_ == d_, a == 0, b_ == tid)
                    ts[4][pat] += 1
                    c["shdr_pos_mod_%d" % (pos % 4)] += 1
                else:
                    if st == b"kapR":
                        clen = struct.unpack_from("<I", d, pos + 12)[0]
                        unc = int.from_bytes(d[pos + 18:pos + 21], "big")
                        rpak_unc[unc if unc in (0x20000, 0x1FFF4, 0x10000) else "other"] += 1
                        c["rpak_len_field_le_body"] += clen <= size - 16
                        c["rpak_pad:%d" % (size - 16 - clen)] += 1
                    elif st == b"TADS":
                        c["sdat_full" if size == 0x20000 else "sdat_part"] += 1
                    else:
                        c["subtype:" + st.hex()] += 1
            if end is not None:
                c["end_eq_len"] += end == len(d)
                if end != len(d):
                    rest = d[end:]
                    c["trailing_zero" if not any(rest) else "trailing_nonzero"] += 1
    for k in sorted(c):
        print(f"{k:28} {c[k]}")
    print("fill sizes", fill_sizes.most_common(12))
    print("fill body", fill_body)
    print("rpak uncompressed sizes", rpak_unc)
    print("\ntype_id   count   bytes        f0 values / alignments / id patterns(a==b,b==c,c==d,a==0,b==tid)")
    for tid, ts in sorted(type_stats.items(), key=lambda kv: -kv[1][0]):
        print(f"{tid:08x} {ts[0]:7} {ts[1]:12} {[(hex(k), v) for k, v in ts[2].most_common(3)]} {[(hex(k), v) for k, v in ts[3].most_common(3)]} {ts[4].most_common(3)}")
    print(examples)


if __name__ == "__main__":
    main(sys.argv[1])

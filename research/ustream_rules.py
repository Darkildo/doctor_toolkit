#!/usr/bin/env python3
"""Checks the FILL placement rule, Rpak chunk sizes and recovers type tag names.

Usage: ustream_rules.py <game_dir>
"""
import collections
import itertools
import os
import re
import struct
import sys

PAGE = 0x20000


def sdbm(s):
    h = 0
    for ch in s:
        h = (ch + h * 65599) & 0xFFFFFFFF
    return h


def main(root):
    c = collections.Counter()
    unc_hist = collections.Counter()
    in_hist = collections.Counter()
    tids, pols = set(), set()
    gap_after = collections.Counter()
    for dp, _, fns in os.walk(root):
        for fn in fns:
            if not fn.lower().endswith(".str"):
                continue
            d = open(os.path.join(dp, fn), "rb").read()
            pos = 12
            blocks = []
            while pos + 8 <= len(d):
                tag, size = struct.unpack_from("<4sI", d, pos)
                blocks.append((pos, tag, size, d[pos + 8:pos + 12]))
                pos += size
            for i, (pos, tag, size, st) in enumerate(blocks):
                if tag == b"LLIF":
                    if i + 1 < len(blocks):
                        nsize = blocks[i + 1][2]
                        c["midfill"] += 1
                        c["midfill_next_would_fit"] += nsize <= size
                        c["midfill_next_is_" + blocks[i + 1][3][::-1].decode()] += 1
                        gap_after[blocks[i - 1][3][::-1].decode()] += 1
                    continue
                rem = PAGE - (pos + size) % PAGE if (pos + size) % PAGE else 0
                if 0 < rem < 8:
                    c["remainder_lt_8"] += 1
                if st == b"RDHS":
                    f0, al, tid = struct.unpack_from("<3I", d, pos + 12)
                    tids.add(tid)
                    pols.add(f0)
                    if i + 1 < len(blocks) and blocks[i + 1][1] == b"LLIF":
                        c["shdr_then_fill"] += 1
                elif st == b"kapR":
                    clen = struct.unpack_from("<I", d, pos + 12)[0]
                    fb = d[pos + 16]
                    hl = 5 if fb & 1 else 2
                    unc = int.from_bytes(d[pos + 16 + hl:pos + 19 + hl], "big")
                    c["rpak_flags:%02x%02x" % (fb, d[pos + 17])] += 1
                    unc_hist[unc.bit_length()] += 1
                    c["rpak_max_unc"] = max(c["rpak_max_unc"], unc)
                    c["rpak_ends_page"] += (pos + size) % PAGE == 0
                    c["rpak_unc_mod4:%d" % (unc % 4)] += 1
                elif st == b"TADS":
                    c["sdat_ends_page"] += (pos + size) % PAGE == 0
    for k in sorted(c):
        print(f"{k:28} {c[k]}")
    print("rpak unc bit_length hist", sorted(unc_hist.items()))
    print("block before mid FILL", gap_after)

    # Recover tag names: SDBM over short strings and strings found in the game executable.
    want = tids | pols
    found = {}
    alpha = b"abcdefghijklmnopqrstuvwxyz0123456789_"
    for n in range(1, 5):
        for t in itertools.product(alpha, repeat=n):
            s = bytes(t)
            h = sdbm(s)
            if h in want:
                found.setdefault(h, []).append(s.decode())
    exe = os.path.join(root, "godfather2.exe")
    if os.path.exists(exe):
        blob = open(exe, "rb").read()
        words = set(re.findall(rb"[A-Za-z0-9_.:]{3,40}", blob))
        print("exe strings", len(words))
        for w in words:
            for v in {w, w.lower()}:
                h = sdbm(v)
                if h in want:
                    found.setdefault(h, []).append(v.decode())
    print("types", len(tids), "policies", len(pols))
    for h in sorted(want):
        kind = "policy" if h in pols else "type"
        print(f"{kind:6} {h:08x} {sorted(set(found.get(h, [])))[:6]}")


if __name__ == "__main__":
    main(sys.argv[1])

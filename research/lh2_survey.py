#!/usr/bin/env python3
"""Checks the LCH2 localisation layout on resources dumped with `doctor-toolkit dump --type lh2`.

Usage: lh2_survey.py <dump_dir>
"""
import collections
import os
import struct
import sys


def main(root):
    c = collections.Counter()
    for dp, _, fns in os.walk(root):
        for fn in fns:
            d = open(os.path.join(dp, fn), "rb").read()
            magic, size, ver, flags, lang_off, n, m, z1, z2 = struct.unpack_from("<4sIHHIIIII", d, 0)
            offs = struct.unpack_from("<%dI" % (n * m), d, 0x20 + 4 * n)
            pool = 0x20 + 4 * n + 4 * n * m
            pos = pool
            contiguous = True
            seen = {}
            dedup = 0
            for o in offs:
                s = d[o:d.index(b"\0", o)]
                if o == pos:
                    pos += len(s) + 1
                elif o < pos:
                    dedup += 1
                    contiguous = contiguous and seen.get(s) == o
                else:
                    contiguous = False
                seen.setdefault(s, o)
            c["files"] += 1
            c["contiguous_or_reused"] += contiguous
            c["reused_offsets"] += dedup
            c["dedup_is_first_occurrence"] += all(seen[d[o:d.index(b'\0', o)]] == o for o in offs)
            end = lang_off if lang_off else len(d)
            c["pool_end_eq_lang_or_end"] += pos == end
            c["padding_after_pool:%d" % (end - pos)] += 1
            if lang_off:
                c["lang_table_len_ok"] += lang_off + 8 * m == len(d)
            c["utf8_ok"] += all(_utf8(d[o:d.index(b"\0", o)]) for o in offs)
    for k in sorted(c):
        print(f"{k:32} {c[k]}")


def _utf8(b):
    try:
        b.decode("utf-8")
        return True
    except UnicodeDecodeError:
        return False


if __name__ == "__main__":
    main(sys.argv[1])

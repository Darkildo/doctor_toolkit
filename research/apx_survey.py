#!/usr/bin/env python3
"""Survey of apx (Flash UI) resources dumped with `doctor-toolkit dump --type apx`.

Usage: apx_survey.py <dump_dir>
"""
import collections
import os
import struct
import sys


def main(root):
    c = collections.Counter()
    tags = collections.Counter()
    seq = collections.Counter()
    sample = {}
    for dp, _, fns in os.walk(root):
        for fn in fns:
            d = open(os.path.join(dp, fn), "rb").read()
            magic, total = struct.unpack_from("<4sI", d, 0)
            c["magic:" + magic[::-1].decode()] += 1
            c["total==len"] += total == len(d)
            pos = 8
            order = []
            while pos < len(d):
                t, size = struct.unpack_from("<4sI", d, pos)
                tag = t[::-1].decode("latin1")
                tags[tag] += 1
                order.append(tag)
                sample.setdefault(tag, (fn, d[pos + 8:pos + 8 + min(size - 8, 40)]))
                if size < 8:
                    c["bad size"] += 1
                    break
                pos += size
            c["ends_exactly"] += pos == len(d)
            seq[" ".join(order[:6])] += 1
    for k in sorted(c):
        print(f"{k:20} {c[k]}")
    print(tags)
    for s, n in seq.most_common(6):
        print(n, s)
    for t, (fn, b) in sample.items():
        print(t, fn, b)


if __name__ == "__main__":
    main(sys.argv[1])

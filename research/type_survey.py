#!/usr/bin/env python3
"""Survey of resource types not yet handled by the library.

Run `doctor-toolkit dump <game> --type <tag> -o <dump>/<tag>` for every type
of interest first; this script walks `<dump>/<tag>/**` and prints, per type:
resource count, size statistics, the most common first 4 bytes (as hex and
ASCII) and an example file.

Usage: type_survey.py <dump_dir> [--markdown]
"""
import collections
import os
import statistics
import sys


def ascii4(b):
    return "".join(chr(c) if 32 <= c < 127 else "." for c in b)


def survey(root):
    rows = []
    for tag in sorted(os.listdir(root)):
        sizes = []
        magics = collections.Counter()
        example = None
        for dp, _, fns in os.walk(os.path.join(root, tag)):
            for fn in fns:
                p = os.path.join(dp, fn)
                with open(p, "rb") as f:
                    head = f.read(4)
                sizes.append(os.path.getsize(p))
                magics[head] += 1
                example = example or os.path.relpath(p, root)
        if not sizes:
            continue
        rows.append((tag, sizes, magics, example))
    rows.sort(key=lambda r: -len(r[1]))
    return rows


def main(root, markdown):
    rows = survey(root)
    if markdown:
        print("| Тег | Ресурсов | Мин. | Медиана | Макс. | Всего | Первые 4 байта (доля) |")
        print("|---|---:|---:|---:|---:|---:|---|")
    for tag, sizes, magics, example in rows:
        top = ", ".join(
            f"`{m.hex()}` {ascii4(m)!r} {n * 100 // len(sizes)}%" for m, n in magics.most_common(3)
        )
        stats = (min(sizes), int(statistics.median(sizes)), max(sizes), sum(sizes))
        if markdown:
            print(f"| `{tag}` | {len(sizes)} | {stats[0]} | {stats[1]} | {stats[2]} | {stats[3]} | {top} |")
        else:
            print(f"{tag:26} n={len(sizes):6} min={stats[0]:8} med={stats[1]:8} max={stats[2]:9} sum={stats[3]:10}")
            print(f"{'':26} {top}")
            print(f"{'':26} e.g. {example}")


if __name__ == "__main__":
    main(sys.argv[1], "--markdown" in sys.argv[2:])

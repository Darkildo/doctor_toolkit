#!/usr/bin/env python3
"""Stage 100: the table of the timing series (timing.sh).

    timing_summary.py OUT_DIR        (the folder with t-MODE-N/timing.txt)

Lines of timing.txt:
    load  T_LEVEL T_FRAMES  READS BYTES MS  COMMITTED_MB TEXTURES_MB  ZONES
    NAME  SETTLE_S          READS BYTES MS  COMMITTED_MB TEXTURES_MB  ZONES
(ZONES: loaded streams with `\\local\\` in their path)
    stage SECONDS BYTES
For every mode prints, per row, the values of every repeat and their range
(minimum – maximum): no averages over three runs.
"""
import collections
import glob
import os
import re
import sys


def spread(values, digits=1, scale=1.0):
    values = [v * scale for v in values]
    if not values:
        return "—"
    each = " / ".join(f"{v:.{digits}f}" for v in values)
    return f"{min(values):.{digits}f}–{max(values):.{digits}f} ({each})"


def main():
    out = sys.argv[1]
    data = collections.defaultdict(lambda: collections.defaultdict(list))
    order = []
    for path in sorted(glob.glob(os.path.join(out, "t-*-*", "timing.txt"))):
        mode = re.match(r"t-(\w+)-\d+", os.path.basename(os.path.dirname(path)))[1]
        for line in open(path):
            parts = line.replace(",", ".").split()
            if not parts:
                continue
            name = parts[0]
            if name not in order:
                order.append(name)
            data[mode][name].append([float(x) for x in parts[1:]])
    modes = [m for m in ("none", "rebuilt", "memory") if m in data]
    for mode in modes:
        print(f"== {mode} ({len(data[mode].get('load', []))} run(s))")
        for name in order:
            rows = data[mode].get(name, [])
            if not rows:
                continue
            if name == "stage":
                print(f"  staging: {spread([r[0] for r in rows], 2)} s, "
                      f"{spread([r[1] for r in rows], 1, 1e-6)} MB of real files")
                continue
            if name == "load":
                head = (f"level streams after {spread([r[0] for r in rows])} s, "
                        f"frame 120 after {spread([r[1] for r in rows])} s")
                rest = [r[2:] for r in rows]
            else:
                head = f"streams settle in {spread([r[0] for r in rows])} s"
                rest = [r[1:] for r in rows]
            print(f"  {name}: {head}")
            print(f"      reads {spread([r[0] for r in rest], 0)}, "
                  f"{spread([r[1] for r in rest], 1, 1e-6)} MB, "
                  f"in ReadFile {spread([r[2] for r in rest], 1)} ms; "
                  f"committed {spread([r[3] for r in rest], 0)} MB, "
                  f"textures {spread([r[4] for r in rest], 0)} MB; "
                  f"zone streams loaded {spread([r[5] for r in rest], 0)}")


if __name__ == "__main__":
    main()

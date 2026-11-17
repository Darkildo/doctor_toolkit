#!/usr/bin/env python3
"""Counts the SimGroup object records of the game by the first flag byte
(`+0x1C`): bit 0 — the record is packed (class hash at `+0x28`, packets as
masks and values), bit 1 — it has the packet `Base`, bit 3 — it has a
component list (stage 80, docs/analysis/asi-platform.md §2).

The iterator of the engine (`0x0043AFF0`) has a second path for records
without bit 0 — a list of tagged entries from `+0x20`; this script shows
whether the files of the game hold such records.

Usage:
    record_modes.py <simgroup_dump_dir>     # `doctor-toolkit dump --type simgroup`
"""
import collections
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from class_usage import groups  # noqa: E402


def main(root):
    flags = collections.Counter()
    files = 0
    for _, d, offs, _ in groups(root):
        files += 1
        for o in offs:
            flags[d[o + 0x1C]] += 1
    total = sum(flags.values())
    print("simgroups: %d, objects: %d" % (files, total))
    for value, n in sorted(flags.items()):
        print("flags 0x%02x: %6d  packed=%d base=%d components=%d"
              % (value, n, value & 1, value >> 1 & 1, value >> 3 & 1))
    print("unpacked records (bit 0 clear): %d" % sum(n for v, n in flags.items() if not v & 1))


if __name__ == "__main__":
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    main(sys.argv[1])

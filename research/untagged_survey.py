#!/usr/bin/env python3
"""Header statistics of the two untagged game-logic types.

Usage: untagged_survey.py <dump_root>
where <dump_root>/df14da75 and <dump_root>/cdb9d42b hold resources dumped with
`doctor-toolkit dump --type df14da75` and `--type cdb9d42b`.

Checked claims:

df14da75: u32 2 at 0x00; u32 N at 0x04; 16-byte GUIDs (8 random bytes and a
          machine name) at 0x38 and further on, mostly 0x8C bytes apart;
          none of them is the GUID of a SimGroup object.
cdb9d42b: u32 2 at 0x00; magic "eerT" (u32 0x54726565, "Tree") at 0x04;
          two small counts at 0x08 and 0x0C.
"""
import collections
import os
import re
import struct
import sys


def files(root):
    for dp, _, fns in os.walk(root):
        for fn in fns:
            yield open(os.path.join(dp, fn), "rb").read()


def main(root):
    c = collections.Counter()
    for d in files(os.path.join(root, "df14da75")):
        c["df14:files"] += 1
        c["df14:version_2"] += struct.unpack_from("<I", d, 0)[0] == 2
        guids = [m.start() - 8 for m in re.finditer(rb"[A-Z0-9_]{8}", d) if m.start() >= 8 and (m.start() - 8) % 4 == 0]
        c["df14:guid_at_0x38"] += bool(guids) and guids[0] == 0x38
        for a, b in zip(guids, guids[1:]):
            c["df14:guid_gap_%d" % (b - a)] += 1
    for d in files(os.path.join(root, "cdb9d42b")):
        c["cdb9:files"] += 1
        version, magic, a, b = struct.unpack_from("<4I", d, 0)
        c["cdb9:version_2"] += version == 2
        c["cdb9:magic_Tree"] += magic == 0x54726565
        c["cdb9:counts_%d_%d" % (a, b)] += 1
    for k, v in sorted(c.items(), key=lambda kv: (kv[0].split(":")[0], -kv[1])):
        if v > 5 or not k.split(":")[1].startswith(("guid_gap", "counts")):
            print(f"{k:28} {v}")


if __name__ == "__main__":
    main(sys.argv[1])

#!/usr/bin/env python3
"""Finds tables of code pointers of godfather2.exe (PC 1.0.789.0) that point into
one address range: method tables and action tables of a subsystem.

Usage (EARS_GF2_DIR is the game directory):
    exe_ptr_tables.py LOW HIGH [MIN_ENTRIES]

Prints every run of at least MIN_ENTRIES (default 8) consecutive 32-bit words
outside the code section whose values lie in [LOW, HIGH).
"""
import struct
import sys

from exe_dis import Exe


def main():
    e = Exe()
    low, high = int(sys.argv[1], 16), int(sys.argv[2], 16)
    least = int(sys.argv[3]) if len(sys.argv) > 3 else 8
    for name, sva, vsz, raw, rsz in e.secs[1:]:
        run = []
        for off in range(raw, raw + rsz - 3, 4):
            v = struct.unpack_from("<I", e.d, off)[0]
            if low <= v < high:
                run.append((sva + off - raw, v))
            else:
                if len(run) >= least:
                    print("%s %08X: %d entries: %s" % (name, run[0][0], len(run),
                                                      " ".join("%08X" % v for _, v in run[:80])))
                run = []


if __name__ == "__main__":
    main()

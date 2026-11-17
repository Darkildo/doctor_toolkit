#!/usr/bin/env python3
"""The "variables" of face-to-face dialogues in godfather2.exe (PC 1.0.789.0):
decodes the switches of the functions that read and write them (stage 75).

Usage (EARS_GF2_DIR is the game directory):
    dialogue_exe.py            every variable of every accessor: where its case is and
                               what the case calls (addresses, immediates, offsets)
    dialogue_exe.py NAME VAR   the full disassembly of one case, e.g. `get_bool 0x0e`

Accessors: get_bool 0x006EBFF0 and set_bool 0x006EB980 (its switch index is the
variable minus 3). set_int 0x006EBDD0 has no switch: it does nothing.
"""
import re
import struct
import sys

from exe_dis import Exe

ACCESSORS = {
    "get_bool": 0x006EBFF0,
    "set_bool": 0x006EB980,
}


def find_switch(e, start, limit=0x400):
    """The first `cmp reg, N; ja …; [movzx reg, byte [reg+BT]]; jmp [reg*4+JT]` of a function."""
    lines = e.dis(start, limit // 2)
    count = None
    bt = None
    base = 0
    for line in lines:
        m = re.search(r"lea\s+e\w+, \[e\w+ - (0x[0-9a-f]+|\d+)\]", line)
        if m and count is None:
            base = int(m.group(1), 0)
        m = re.search(r"cmp\s+e\w+, (0x[0-9a-f]+|\d+)$", line)
        if m and count is None:
            count = int(m.group(1), 0) + 1
        m = re.search(r"movzx\s+e\w+, byte ptr \[e\w+ \+ (0x[0-9a-f]+)\]", line)
        if m and count is not None:
            bt = int(m.group(1), 16)
        m = re.search(r"jmp\s+dword ptr \[e\w+\*4 \+ (0x[0-9a-f]+)\]", line)
        if m and count is not None:
            return count, bt, int(m.group(1), 16), base
    return None


def cases(e, start):
    sw = find_switch(e, start)
    if sw is None:
        return {}
    count, bt, jt, base = sw
    out = {}
    for v in range(count):
        k = e.d[e.off(bt) + v] if bt else v
        out[v + base] = e.u32(jt + 4 * k)
    return out


def brief(e, va, limit=60):
    """Calls, immediates and member offsets of one case, up to its return."""
    seen = []
    for line in e.dis(va, limit):
        m = re.search(r"call\s+(0x[0-9a-f]+)", line)
        if m:
            seen.append("call " + m.group(1))
        m = re.search(r"push\s+(0x[0-9a-f]{3,}|\d+)$", line)
        if m:
            seen.append("push " + m.group(1))
        m = re.search(r"\[e\w+ \+ (0x[0-9a-f]{3,})\]", line)
        if m and "esp" not in line:
            seen.append("+" + m.group(1))
        m = re.search(r"call\s+(e\w+|dword ptr \[e\w+ \+ 0x[0-9a-f]+\])", line)
        if m:
            seen.append("call " + m.group(1).replace("dword ptr ", ""))
        if re.search(r"\s(ret|jmp)\s", line + " "):
            break
    return " ".join(seen)


def main():
    e = Exe()
    if len(sys.argv) == 3:
        va = cases(e, ACCESSORS[sys.argv[1]])[int(sys.argv[2], 0)]
        print("\n".join(e.dis(va, 400, True)))
        return
    for name, start in ACCESSORS.items():
        table = cases(e, start)
        default = max(set(table.values()), key=list(table.values()).count) if table else None
        print("== %s 0x%08X: %d variables, default case 0x%08X" % (name, start, len(table), default or 0))
        for v, va in table.items():
            if va != default:
                print("  0x%02x  %08X  %s" % (v, va, brief(e, va)))


if __name__ == "__main__":
    main()

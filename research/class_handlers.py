#!/usr/bin/env python3
"""Finds the attribute reader and the message handler of SimGroup classes in
godfather2.exe (PC 1.0.789.0) from the class name alone, without Ghidra.

Usage:
    EARS_GF2_DIR=<game dir> class_handlers.py MissionTimer EnterExitTrigger ...

How (docs/logic-graph.md §9, docs/analysis/story.md §2):

1. the reader of a class walks its packet with `0x43AFF0(packet, hash)`, the
   hash being `sdbm(lower(name))`: the place that pushes the hash before that
   call lies in the reader;
2. the reader is slot 1 of the method table of the object (slot 0 is the
   deleting destructor): the table is the word that holds the reader's address;
3. the constructor stores the object table at `[obj]` and the table of the
   message listener at `[obj+0x3C]`; the handler is slot 1 of the listener table.

Prints, per class: hash, reader, object table, listener table, handler. `?`
where a step finds nothing or more than one candidate (they are listed).
"""
import struct
import sys

from exe_xrefs import Exe

WALK = 0x43AFF0


def sdbm_lower(name):
    h = 0
    for c in name.lower().encode():
        h = (c + (h << 6) + (h << 16) - h) & 0xFFFFFFFF
    return h


def main():
    e = Exe()
    code, tb = e.code, e.tb
    for name in sys.argv[1:]:
        h = sdbm_lower(name)
        readers = set()
        i = 0
        push = b"\x68" + struct.pack("<I", h)
        while True:
            i = code.find(push, i)
            if i < 0:
                break
            # the call of the packet walk follows within a few instructions
            for j in range(i + 5, i + 24):
                if code[j] == 0xE8 and tb + j + 5 + struct.unpack_from("<i", code, j + 1)[0] == WALK:
                    readers.add(e.function(tb + i))
            i += 1
        print(f"{name}: hash 0x{h:08x}")
        for r in sorted(readers):
            tables = [va - 4 for va, kind in e.refs(r) if kind == "word" and not (tb <= va < tb + len(code))]
            print(f"  reader 0x{r:08X}; object table(s): {', '.join(f'0x{t:08X}' for t in tables) or '?'}")
            for t in tables:
                # constructors and destructors: mov dword [reg], table ... mov dword [reg+0x3C], listener
                seen = set()
                w = struct.pack("<I", t)
                k = 0
                while True:
                    k = code.find(w, k)
                    if k < 0:
                        break
                    window = code[k:k + 48]
                    for m in range(len(window) - 7):
                        if window[m] == 0xC7 and window[m + 1] & 0xF8 == 0x40 and window[m + 2] == 0x3C:
                            seen.add(struct.unpack_from("<I", window, m + 3)[0])
                    k += 1
                for lt in sorted(seen):
                    o = e.off(lt)
                    handler = struct.unpack_from("<I", e.d, o + 4)[0]
                    print(f"    listener table 0x{lt:08X}: handler 0x{handler:08X}")
                if not seen:
                    print("    listener table: ?")
        if not readers:
            print("  reader: ?")


if __name__ == "__main__":
    main()

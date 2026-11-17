#!/usr/bin/env python3
"""Imports of godfather2.exe by DLL (stage 81).

Usage:
    EARS_GF2_DIR=<game> python3 research/exe_imports.py [DLL]

Prints every DLL the executable imports with the number of functions, or,
with DLL (e.g. dinput8.dll), the functions it imports from that one. An ASI
loader takes the place of one of these DLLs: the fewer functions the game
takes from it, the less the loader has to pass on. PC 1.0.789.0 imports one
function of DINPUT8.dll, DirectInput8Create.
"""
import os
import struct
import sys


def main():
    game = os.environ.get("EARS_GF2_DIR")
    if not game:
        sys.exit("EARS_GF2_DIR is not set")
    data = open(os.path.join(game, "godfather2.exe"), "rb").read()
    pe = struct.unpack_from("<I", data, 0x3C)[0]
    sections = struct.unpack_from("<H", data, pe + 6)[0]
    optional = pe + 24
    size_optional = struct.unpack_from("<H", data, pe + 20)[0]
    import_rva = struct.unpack_from("<I", data, optional + 96 + 8)[0]
    table = optional + size_optional

    def offset(rva):
        for i in range(sections):
            virtual_size, virtual, raw_size, raw = struct.unpack_from("<IIII", data, table + i * 40 + 8)
            if virtual <= rva < virtual + max(virtual_size, raw_size):
                return raw + rva - virtual
        raise ValueError("RVA %#x is in no section" % rva)

    def text(rva):
        at = offset(rva)
        return data[at:data.index(b"\0", at)].decode("latin-1")

    wanted = sys.argv[1].lower() if len(sys.argv) > 1 else None
    at = offset(import_rva)
    while True:
        lookup, _, _, name, thunks = struct.unpack_from("<IIIII", data, at)
        if not name:
            break
        at += 20
        dll = text(name)
        functions = []
        entry = offset(lookup or thunks)
        while True:
            value = struct.unpack_from("<I", data, entry)[0]
            if not value:
                break
            entry += 4
            functions.append("#%d" % (value & 0xFFFF) if value & 0x80000000 else text(value + 2))
        if wanted is None:
            print("%-16s %d" % (dll, len(functions)))
        elif dll.lower() == wanted:
            for function in functions:
                print(function)


if __name__ == "__main__":
    main()

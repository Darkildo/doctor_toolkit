#!/usr/bin/env python3
"""Stage 100: where godfather2.exe (PC 1.0.789.0) calls the KERNEL32 file
functions by handle (ReadFile, SetFilePointer[Ex], GetFileSize[Ex], ...).

For every import of interest prints its slot and each instruction in .text
that names the slot (`call [slot]`, `mov reg, [slot]`, `jmp [slot]`), found
by the four bytes of the slot address and checked with the disassembler.

    EARS_GF2_DIR=<game folder> python3 research/file_io_sites.py [NAME ...]
    EARS_GF2_DIR=<game folder> python3 research/file_io_sites.py --dis ADDR [COUNT]
"""
import os
import struct
import sys

import capstone
import pefile

NAMES = [
    "ReadFile", "ReadFileEx", "WriteFile", "SetFilePointer", "SetFilePointerEx",
    "GetFileSize", "GetFileSizeEx", "CloseHandle", "GetOverlappedResult",
    "CreateFileMappingA", "CreateFileMappingW", "MapViewOfFile",
    "GetFileInformationByHandle", "GetFileType", "DuplicateHandle",
    "CreateIoCompletionPort", "GetQueuedCompletionStatus", "SetEndOfFile",
    "FlushFileBuffers", "GetFileTime", "DeviceIoControl",
]


def load():
    exe = os.path.join(os.environ["EARS_GF2_DIR"], "godfather2.exe")
    pe = pefile.PE(exe, fast_load=True)
    pe.parse_data_directories(
        directories=[pefile.DIRECTORY_ENTRY["IMAGE_DIRECTORY_ENTRY_IMPORT"]])
    return pe


def disassemble(pe, address, count):
    """Prints `count` instructions from `address`, naming imports."""
    names = {}
    for entry in pe.DIRECTORY_ENTRY_IMPORT:
        for imp in entry.imports:
            if imp.name:
                names[imp.address] = imp.name.decode()
    image = pe.OPTIONAL_HEADER.ImageBase
    data = pe.get_data(address - image, count * 8)
    md = capstone.Cs(capstone.CS_ARCH_X86, capstone.CS_MODE_32)
    for n, ins in enumerate(md.disasm(data, address)):
        if n >= count:
            break
        note = ""
        for slot, name in names.items():
            if f"{slot:#x}" in ins.op_str:
                note = f"   ; {name}"
        print(f"{ins.address:#010x}  {ins.bytes.hex():<20} {ins.mnemonic} {ins.op_str}{note}")


def main():
    pe = load()
    if len(sys.argv) > 2 and sys.argv[1] == "--dis":
        count = int(sys.argv[3]) if len(sys.argv) > 3 else 60
        disassemble(pe, int(sys.argv[2], 16), count)
        return
    wanted = sys.argv[1:] or NAMES
    slots = {}
    for entry in pe.DIRECTORY_ENTRY_IMPORT:
        if entry.dll.lower() != b"kernel32.dll":
            continue
        for imp in entry.imports:
            if imp.name and imp.name.decode() in wanted:
                slots[imp.name.decode()] = imp.address
    text = next(s for s in pe.sections if s.Name.startswith(b".text"))
    base = pe.OPTIONAL_HEADER.ImageBase + text.VirtualAddress
    code = text.get_data()
    md = capstone.Cs(capstone.CS_ARCH_X86, capstone.CS_MODE_32)
    for name in wanted:
        slot = slots.get(name)
        if slot is None:
            print(f"{name}: not imported")
            continue
        print(f"{name}: slot {slot:#010x}")
        needle = struct.pack("<I", slot)
        at = code.find(needle)
        while at != -1:
            # The instruction starts one or two bytes before the address.
            for back in (2, 1):
                start = at - back
                ins = next(md.disasm(code[start:start + 16], base + start), None)
                if ins and ins.size == back + 4 and f"{slot:#x}" in ins.op_str:
                    print(f"  {ins.address:#010x}  {ins.bytes.hex()}  {ins.mnemonic} {ins.op_str}")
                    break
            at = code.find(needle, at + 1)


if __name__ == "__main__":
    main()

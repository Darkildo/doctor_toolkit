#!/usr/bin/env python3
"""Finds references to addresses in godfather2.exe (PC 1.0.789.0) without Ghidra.

Usage:
    EARS_GF2_DIR=<game dir> exe_xrefs.py refs 0x00AC3290 [...]   # calls, jumps and 32-bit words equal to the address
    EARS_GF2_DIR=<game dir> exe_xrefs.py dis 0x00AC3290 [count]  # disassembly (needs `capstone`)
    EARS_GF2_DIR=<game dir> exe_xrefs.py str "text"              # addresses of a string and references to them
    EARS_GF2_DIR=<game dir> exe_xrefs.py words 0x00E4D3E0 [n]    # 32-bit words at an address

`refs` prints `call`/`jmp` for relative branches and `word` for a 32-bit word
with the value (an immediate operand, a method table slot, a data pointer);
for each one, the nearest 16-byte aligned address preceded by padding or a
return — a guide to the function, not a proven start. The executable is only read.
"""
import hashlib
import os
import struct
import sys

SHA256 = "31f579a80cfd14f42c0379bbfa5b5b7f151bb06ec5163593023ab5002b1f1286"


class Exe:
    def __init__(self):
        path = os.path.join(os.environ["EARS_GF2_DIR"], "godfather2.exe")
        self.d = d = open(path, "rb").read()
        if hashlib.sha256(d).hexdigest() != SHA256:
            sys.exit("godfather2.exe is not PC 1.0.789.0")
        pe = struct.unpack_from("<I", d, 0x3C)[0]
        n = struct.unpack_from("<H", d, pe + 6)[0]
        opt = struct.unpack_from("<H", d, pe + 20)[0]
        self.base = struct.unpack_from("<I", d, pe + 52)[0]
        self.secs = []
        o = pe + 24 + opt
        for _ in range(n):
            name = d[o:o + 8].rstrip(b"\0").decode()
            vs, va, rs, ro = struct.unpack_from("<IIII", d, o + 8)
            self.secs.append((name, self.base + va, vs, ro, rs))
            o += 40
        t = next(s for s in self.secs if s[0] == ".text")
        self.tb, self.code = t[1], d[t[3]:t[3] + t[4]]

    def off(self, va):
        for _, a, _, ro, rs in self.secs:
            if a <= va < a + rs:
                return ro + va - a
        return None

    def va(self, off):
        for _, a, _, ro, rs in self.secs:
            if ro <= off < ro + rs:
                return a + off - ro
        return None

    def function(self, site):
        code = self.code
        if not self.tb <= site < self.tb + len(self.code):
            return 0
        a = (site - self.tb) & ~0xF
        while a > 0:
            if code[a - 1] in (0xCC, 0x90, 0xC3) or code[a - 3] == 0xC2:
                return self.tb + a
            a -= 16
        return self.tb

    def refs(self, target):
        out = []
        code = self.code
        for op, kind in ((b"\xe8", "call"), (b"\xe9", "jmp")):
            i = 0
            while True:
                i = code.find(op, i)
                if i < 0:
                    break
                if i + 5 <= len(code) and self.tb + i + 5 + struct.unpack_from("<i", code, i + 1)[0] == target:
                    out.append((self.tb + i, kind))
                i += 1
        w = struct.pack("<I", target)
        i = 0
        while True:
            i = self.d.find(w, i)
            if i < 0:
                break
            va = self.va(i)
            if va is not None:
                out.append((va, "word"))
            i += 1
        return sorted(out)

    def dis(self, va, count):
        import capstone
        md = capstone.Cs(capstone.CS_ARCH_X86, capstone.CS_MODE_32)
        o = self.off(va)
        for k, ins in enumerate(md.disasm(self.d[o:o + count * 15], va)):
            if k >= count:
                break
            print(f"{ins.address:08x}  {ins.mnemonic} {ins.op_str}")


def main():
    a = sys.argv[1:]
    if len(a) < 2:
        sys.exit(__doc__)
    e = Exe()
    if a[0] == "refs":
        for t in a[1:]:
            t = int(t, 16)
            for va, kind in e.refs(t):
                where = f"in ~0x{e.function(va):08X}" if e.tb <= va < e.tb + len(e.code) else "data"
                print(f"0x{t:08X} <- 0x{va:08X} {kind} {where}")
    elif a[0] == "dis":
        e.dis(int(a[1], 16), int(a[2]) if len(a) > 2 else 40)
    elif a[0] == "str":
        s = a[1].encode() + b"\0"
        i = 0
        while True:
            i = e.d.find(s, i)
            if i < 0:
                break
            va = e.va(i)
            if va is not None and e.d[i - 1] == 0:
                print(f"string at 0x{va:08X}")
                for r, kind in e.refs(va):
                    print(f"  <- 0x{r:08X} {kind} in ~0x{e.function(r):08X}")
            i += 1
    elif a[0] == "words":
        va = int(a[1], 16)
        o = e.off(va)
        for k in range(int(a[2]) if len(a) > 2 else 8):
            print(f"0x{va + 4 * k:08X}: 0x{struct.unpack_from('<I', e.d, o + 4 * k)[0]:08X}")
    else:
        sys.exit(__doc__)


if __name__ == "__main__":
    main()

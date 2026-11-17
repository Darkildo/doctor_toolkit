#!/usr/bin/env python3
"""Small reader of godfather2.exe (PC 1.0.789.0) for work without Ghidra.

Usage (EARS_GF2_DIR is the game directory; the executable is only read):
    exe_dis.py dis VA [COUNT]      disassemble COUNT instructions from VA
    exe_dis.py func VA             disassemble from VA to the padding after a return
    exe_dis.py xref VA             calls, jumps and 32-bit immediates/data words equal to VA
    exe_dis.py str TEXT            addresses of a zero-terminated string and what refers to them
    exe_dis.py cls NAME            where SimGroup class NAME is registered (factory, size)
    exe_dis.py readers NAME        attribute readers of packet NAME: `push sdbm(lower(NAME))`
                                   before a call of 0x43AFF0, with the jump table of the switch
    exe_dis.py dd VA [COUNT]       32-bit words from VA (method tables), with string/code hints
    exe_dis.py imm VALUE           instructions that hold the 32-bit immediate VALUE

Needs `capstone` (`pip install capstone`).
"""
import os
import struct
import sys

import capstone

SHA256 = "31f579a80cfd14f42c0379bbfa5b5b7f151bb06ec5163593023ab5002b1f1286"


class Exe:
    def __init__(self):
        path = os.path.join(os.environ["EARS_GF2_DIR"], "godfather2.exe")
        self.d = open(path, "rb").read()
        pe = struct.unpack_from("<I", self.d, 0x3C)[0]
        nsec = struct.unpack_from("<H", self.d, pe + 6)[0]
        opt = struct.unpack_from("<H", self.d, pe + 20)[0]
        self.base = struct.unpack_from("<I", self.d, pe + 24 + 28)[0]
        self.secs = []
        p = pe + 24 + opt
        for i in range(nsec):
            name = self.d[p:p + 8].rstrip(b"\0").decode()
            vsz, va, rsz, raw = struct.unpack_from("<IIII", self.d, p + 8)
            self.secs.append((name, self.base + va, vsz, raw, rsz))
            p += 40
        self.md = capstone.Cs(capstone.CS_ARCH_X86, capstone.CS_MODE_32)

    def off(self, va):
        for _, sva, vsz, raw, rsz in self.secs:
            if sva <= va < sva + max(vsz, rsz):
                o = va - sva
                return raw + o if o < rsz else None
        return None

    def va(self, off):
        for _, sva, vsz, raw, rsz in self.secs:
            if raw <= off < raw + rsz:
                return sva + off - raw
        return None

    def u32(self, va):
        o = self.off(va)
        return struct.unpack_from("<I", self.d, o)[0] if o is not None else None

    def cstr(self, va, limit=80):
        o = self.off(va)
        if o is None:
            return None
        e = self.d.find(b"\0", o, o + limit)
        s = self.d[o:e if e >= 0 else o + limit]
        if len(s) >= 3 and all(32 <= c < 127 for c in s):
            return s.decode()
        return None

    def text(self):
        name, sva, vsz, raw, rsz = self.secs[0]
        return sva, raw, rsz

    def dis(self, va, count=40, stop_at_ret=False):
        o = self.off(va)
        out = []
        for ins in self.md.disasm(self.d[o:o + count * 16], va):
            hint = ""
            for tok in ins.op_str.replace("[", " ").replace("]", " ").replace(",", " ").split():
                if tok.startswith("0x"):
                    try:
                        v = int(tok, 16)
                    except ValueError:
                        continue
                    s = self.cstr(v)
                    if s and v >= self.base + 0x1000:
                        hint = '  ; "%s"' % s
            out.append("%08X  %-7s %s%s" % (ins.address, ins.mnemonic, ins.op_str, hint))
            if len(out) >= count:
                break
            if stop_at_ret and ins.mnemonic in ("ret", "retn", "jmp"):
                nxt = self.off(ins.address + ins.size)
                if self.d[nxt] in (0xCC, 0x90) or (ins.address + ins.size) % 16 == 0:
                    break
        return out

    def xref(self, target):
        sva, raw, rsz = self.text()
        d = self.d
        res = []
        for i in range(raw, raw + rsz - 5):
            b = d[i]
            if b in (0xE8, 0xE9):
                rel = struct.unpack_from("<i", d, i + 1)[0]
                if sva + (i - raw) + 5 + rel == target:
                    res.append(("call" if b == 0xE8 else "jmp", sva + i - raw))
        pat = struct.pack("<I", target)
        i = d.find(pat)
        while i >= 0:
            v = self.va(i)
            if v is not None:
                res.append(("imm/data", v))
            i = d.find(pat, i + 1)
        return res


def main():
    e = Exe()
    cmd = sys.argv[1]
    if cmd == "dis":
        print("\n".join(e.dis(int(sys.argv[2], 16), int(sys.argv[3]) if len(sys.argv) > 3 else 40)))
    elif cmd == "func":
        print("\n".join(e.dis(int(sys.argv[2], 16), 4000, True)))
    elif cmd == "xref":
        for k, v in e.xref(int(sys.argv[2], 16)):
            print("%-8s %08X" % (k, v))
    elif cmd == "imm":
        pat = struct.pack("<I", int(sys.argv[2], 16))
        i = e.d.find(pat)
        while i >= 0:
            v = e.va(i)
            if v is not None:
                print("%08X" % v)
            i = e.d.find(pat, i + 1)
    elif cmd == "dd":
        va = int(sys.argv[2], 16)
        for k in range(int(sys.argv[3]) if len(sys.argv) > 3 else 16):
            w = e.u32(va + 4 * k)
            s = e.cstr(w) if w else None
            print("%08X  +%02X  %08X%s" % (va + 4 * k, 4 * k, w, '  "%s"' % s if s else ""))
    elif cmd == "readers":
        h = 0
        for c in sys.argv[2].lower().encode():
            h = (c + (h << 6) + (h << 16) - h) & 0xFFFFFFFF
        print("packet %08x" % h)
        pat = b"\x68" + struct.pack("<I", h)
        i = e.d.find(pat)
        while i >= 0:
            v = e.va(i)
            if v is not None and b"\xe8" in e.d[i + 5:i + 16]:
                lines = e.dis(v, 40)
                if any("0x43aff0" in x for x in lines[:5]):
                    print("reader at %08X" % v)
                    for x in lines:
                        if "jmp     dword ptr [e" in x or "cmp     eax" in x or "sub     eax" in x:
                            print("   ", x)
            i = e.d.find(pat, i + 1)
    elif cmd in ("str", "cls"):
        text = sys.argv[2].encode() + b"\0"
        i = e.d.find(b"\0" + text)
        while i >= 0:
            v = e.va(i + 1)
            if v is not None:
                refs = e.xref(v)
                print("string %08X: %s" % (v, ", ".join("%s %08X" % r for r in refs) or "no refs"))
                if cmd == "cls":
                    for _, r in refs:
                        print("\n".join(e.dis(r - 24, 14)))
            i = e.d.find(b"\0" + text, i + 1)


if __name__ == "__main__":
    main()

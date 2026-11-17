#!/usr/bin/env python3
"""Lists every place of godfather2.exe (PC 1.0.789.0) that puts a message on
the bus, and writes data/gf2/bus_sites.tsv.

Usage:
    EARS_GF2_DIR=<game dir> bus_sites.py build > data/gf2/bus_sites.tsv
    EARS_GF2_DIR=<game dir> bus_sites.py check data/gf2/bus_sites.tsv
    EARS_GF2_DIR=<game dir> bus_sites.py stats

Needs the `capstone` disassembler (`pip install capstone`); the executable is
read from the game directory and is never written.

What is collected (see docs/analysis/message-bus.md):

* `name`    a message variable of engine code gets its name:
            `0x408240(variable, "name")` or
            `0x408260(variable, 0x4DAFD0("name"))`;
* `listen`  engine code subscribes a listener to a message:
            `0x408900(listener, variable, flags)`, `0x408680(variable)` and
            `0x407E60(listener, variable)` for a variable that has its hash,
            `0x4089B0(variable, "name")` and `0x473EC0` that also name it, and
            the common tail `0x4084D0(variable, flags)`;
* `send`    broadcast `0x408A00(message, force)` and its wrapper
            `0x45C400(variable, data, force)`;
* `direct`  delivery to one listener: `0x408BF0(message, listener, force)`,
            `0x408B80(variable, listener, force)` (no data) and
            `0x408BB0(variable, listener, data, force)`;
* `delayed` the delayed queue `0x440590(variable, listener, seconds, data,
            flags)` and its wrapper `0x4760D0`.

Each call is read by a symbolic pass over the instructions that precede it
(after the last return or jump elsewhere; every way through the forward jumps
in between is followed): which variable the hash comes from and what is stored
in the data word. A value the pass cannot follow is `?`; nothing is guessed.
A call that sends one of several messages has a row for each.

Columns (tab-separated): kind, site, function, call, message, data, force.
For `name` rows the data column holds the variable. `message` is a name,
`[obj+0x..]` (a variable inside an object: a SimGroup attribute or a member), `var:0x…` (a global variable without a name) or `?`.
`data` is `none` (zero), `obj:0x…` (a data object built on the stack, by the
address of its method table; `data/gf2/messages.toml` names them), `reg`,
`mem`, `?`. `function` is the nearest 16-byte aligned address preceded by
padding or a return; it is a guide for reading, not a proven function start.
"""
import collections
import hashlib
import os
import re
import struct
import sys

SHA256 = "31f579a80cfd14f42c0379bbfa5b5b7f151bb06ec5163593023ab5002b1f1286"

# call target -> index of the argument that is the message variable
LISTEN = {0x408900: 1, 0x4089B0: 0, 0x473EC0: 0, 0x408980: 0, 0x408680: 0, 0x407E60: 1, 0x4084D0: 0}
SEND = 0x408A00
HASH = 0x4DAFD0  # sdbm(lower(name))
INTERN = 0x408260  # (variable, hash): the variable gets the hash
# target -> (kind, argument layout)
WRAPPERS = {
    0x45C400: ("send", {"var": 0, "data": 1, "force": 2}),
    0x408BF0: ("direct", {"msg": 0, "force": 2}),
    0x408B80: ("direct", {"var": 0, "force": 2, "nodata": True}),
    0x408BB0: ("direct", {"var": 0, "data": 2, "force": 3}),
    0x440590: ("delayed", {"var": 0, "data": 3}),
    0x4760D0: ("delayed", {"var": 0, "data": 2}),
}

REG = {"eax", "ebx", "ecx", "edx", "esi", "edi", "ebp"}
SUB = {"al": "eax", "bl": "ebx", "cl": "ecx", "dl": "edx", "ax": "eax", "bx": "ebx",
       "cx": "ecx", "dx": "edx", "si": "esi", "di": "edi", "ah": "eax", "bh": "ebx",
       "ch": "ecx", "dh": "edx"}


def sdbm_lower(name):
    h = 0
    for c in name.lower().encode():
        h = (h * 65599 + c) & 0xFFFFFFFF
    return h


class Exe:
    def __init__(self, path):
        self.d = open(path, "rb").read()
        if hashlib.sha256(self.d).hexdigest() != SHA256:
            sys.exit("godfather2.exe is not edition 1.0.789.0")
        d = self.d
        pe = struct.unpack_from("<I", d, 0x3C)[0]
        nsec = struct.unpack_from("<H", d, pe + 6)[0]
        opt = struct.unpack_from("<H", d, pe + 20)[0]
        base = struct.unpack_from("<I", d, pe + 24 + 28)[0]
        self.secs = []
        o = pe + 24 + opt
        for _ in range(nsec):
            name = d[o:o + 8].rstrip(b"\0").decode()
            vs, va, rs, ro = struct.unpack_from("<IIII", d, o + 8)
            self.secs.append((name, base + va, vs, ro, rs))
            o += 40
        t = next(s for s in self.secs if s[0] == ".text")
        self.tb = t[1]
        self.code = d[t[3]:t[3] + t[4]]
        import capstone
        self.md = capstone.Cs(capstone.CS_ARCH_X86, capstone.CS_MODE_32)

    def off(self, va):
        for _, a, _, ro, rs in self.secs:
            if a <= va < a + rs:
                return ro + va - a
        return None

    def cstr(self, va):
        o = self.off(va)
        if o is None:
            return None
        e = self.d.find(b"\0", o, o + 200)
        if e <= o:
            return None
        s = self.d[o:e]
        if not all(32 <= c < 127 for c in s):
            return None
        return s.decode()

    def calls(self, target):
        out = []
        i = 0
        code = self.code
        while True:
            i = code.find(b"\xe8", i)
            if i < 0:
                break
            if i + 5 <= len(code) and self.tb + i + 5 + struct.unpack_from("<i", code, i + 1)[0] == target:
                out.append(self.tb + i)
            i += 1
        return out

    def function(self, site):
        """Nearest aligned address before `site` that follows padding or a return."""
        code = self.code
        a = (site - self.tb) & ~0xF
        low = max(a - 0x3000, 0)
        while a > low:
            if code[a - 1] in (0xCC, 0xC3, 0x90) or (code[a - 3] == 0xC2 and code[a - 1] == 0):
                at = 0
                for at, _, _, _ in self.md.disasm_lite(code[a:site - self.tb + 5], self.tb + a):
                    if at >= site:
                        break
                if at == site:
                    return self.tb + a
            a -= 16
        return 0

    def constructed(self, ctor):
        """The method table a constructor stores into its object, if it is
        plainly visible in its first instructions."""
        o = ctor - self.tb
        for i in self.md.disasm(self.code[o:o + 64], ctor):
            if i.mnemonic in ("ret", "call", "jmp"):
                break
            m = re.match(r"dword ptr \[e[a-z][a-z]\], (0x[0-9a-f]+)$", i.op_str)
            if i.mnemonic == "mov" and m:
                return ("imm", int(m.group(1), 16))
        return None

    def zero_registers(self, start, end):
        """Callee-saved registers that the function only ever clears before
        `end` (`xor r, r`): the usual way the game keeps a zero at hand."""
        state = {}
        for _, _, mn, ops in self.md.disasm_lite(self.code[start - self.tb:end - self.tb], start):
            parts = [x.strip() for x in ops.split(",")]
            r = parts[0]
            r = SUB.get(r, r)
            if r not in ("ebx", "esi", "edi", "ebp") or mn in ("push", "pop", "cmp", "test"):
                continue
            cleared = mn == "xor" and len(parts) == 2 and parts[0] == parts[1]
            state[r] = cleared and state.get(r, True)
        return {r for r, ok in state.items() if ok}

    def before(self, site, start, count=48):
        # Only the tail is needed: find where it starts with the cheap pass.
        starts = [at for at, _, _, _ in self.md.disasm_lite(self.code[start - self.tb:site - self.tb], start)]
        if not starts:
            return []
        tail = starts[-count] if len(starts) > count else starts[0]
        ins = list(self.md.disasm(self.code[tail - self.tb:site - self.tb], tail))
        if not ins or ins[-1].address + ins[-1].size != site:
            return []
        cut = 0
        for j, i in enumerate(ins):
            if i.mnemonic in ("ret", "int3"):
                cut = j + 1
            elif i.mnemonic == "jmp":
                # A jump over an `else` branch that ends before the call stays:
                # the pass forgets what the two branches wrote.
                t = num(i.op_str)
                if t is None or not i.address < t <= site:
                    cut = j + 1
        return ins[cut:]


def num(s):
    try:
        return int(s.strip(), 0)
    except ValueError:
        return None


def memop(s):
    m = re.match(r"(?:dword|word|byte) ptr \[(.*)\]$", s.strip())
    return m.group(1) if m else None


def espoff(e):
    if e == "esp":
        return 0
    m = re.match(r"esp \+ (0x[0-9a-f]+|\d+)$", e)
    return int(m.group(1), 0) if m else None


def simulate(ins, zero=()):
    """Straight-line pass: registers, stack slots (by offset from the entry
    stack pointer) and the stack pointer at the end; None when the stack
    pointer is lost. `zero` — registers known to hold 0 on entry."""
    regs, st, sp = {r: ("imm", 0) for r in zero}, {}, 0

    def val(o):
        o = o.strip()
        if o in REG:
            return regs.get(o, ("reg", o))
        if o in SUB:
            v = regs.get(SUB[o])
            return v if v == ("imm", 0) else ("?",)
        n = num(o)
        if n is not None:
            return ("imm", n & 0xFFFFFFFF)
        m = memop(o)
        if m is not None:
            n = num(m)
            if n is not None:
                return ("glob", n)
            e = espoff(m)
            if e is not None:
                return st.get(sp + e, ("?",))
            mm = re.match(r"(e[a-z][a-z])(?: \+ (0x[0-9a-f]+|\d+))?$", m)
            if mm:
                base = regs.get(mm.group(1), ("reg", mm.group(1)))
                return ("mem", base, int(mm.group(2) or "0", 0))
            return ("mem", ("?",), 0)
        return ("?",)

    for i in ins:
        mn = i.mnemonic
        ops = [x.strip() for x in i.op_str.split(",")] if i.op_str else []
        if mn == "jmp":
            pass
        elif mn == "push":
            sp -= 4
            st[sp] = val(ops[0])
        elif mn == "pop":
            if ops[0] in REG:
                regs[ops[0]] = st.get(sp, ("?",))
            sp += 4
        elif mn in ("mov", "movzx", "movsx"):
            d, s = ops
            v = val(s) if mn == "mov" else ("?",)
            if d in REG:
                regs[d] = v
            elif d in SUB:
                regs[SUB[d]] = ("?",)
            else:
                m = memop(d)
                e = espoff(m) if m else None
                if e is not None:
                    st[sp + e] = v
        elif mn == "lea":
            d, s = ops
            m = s[1:-1]
            e = espoff(m)
            if e is not None:
                regs[d] = ("lea", sp + e)
            else:
                mm = re.match(r"(e[a-z][a-z])(?: \+ (0x[0-9a-f]+|\d+))?$", m)
                n = num(m)
                if mm:
                    regs[d] = ("addr", regs.get(mm.group(1), ("reg", mm.group(1))), int(mm.group(2) or "0", 0))
                elif n is not None:
                    regs[d] = ("imm", n)
                else:
                    regs[d] = ("?",)
        elif mn == "xor" and len(ops) == 2 and ops[0] == ops[1] and ops[0] in REG:
            regs[ops[0]] = ("imm", 0)
        elif mn in ("add", "sub") and ops[0] == "esp":
            n = num(ops[1])
            if n is None:
                return None
            sp += n if mn == "add" else -n
        elif mn == "call":
            target = num(ops[0])
            hashed = st.get(sp) if target == HASH else None
            this = regs.get("ecx")
            if target is not None and this and this[0] == "lea" and this[1] not in st:
                # A constructor of an object on the stack.
                st[this[1]] = ("ctor", target)
            for r in ("eax", "ecx", "edx"):
                regs[r] = ("?",)
            if hashed and hashed[0] in ("imm", "glob"):
                regs["eax"] = ("hash", hashed)
        elif mn in ("fstp", "fst", "fistp", "movss", "movaps", "movups", "movq", "movd"):
            m = memop(ops[0]) if ops else None
            if m is None and ops:
                mm = re.match(r"(?:xmmword|qword) ptr \[(.*)\]$", ops[0])
                m = mm.group(1) if mm else None
            e = espoff(m) if m else None
            if e is not None:
                st[sp + e] = ("?",)
            elif ops and ops[0] in REG:
                regs[ops[0]] = ("?",)
        elif ops and ops[0] in REG and mn not in ("cmp", "test"):
            regs[ops[0]] = ("?",)
        elif ops and ops[0] in SUB and mn not in ("cmp", "test"):
            regs[SUB[ops[0]]] = ("?",)
        elif ops and mn not in ("cmp", "test", "jmp") and not mn.startswith("j"):
            m = memop(ops[0])
            e = espoff(m) if m else None
            if e is not None:
                st[sp + e] = ("?",)
    return regs, st, sp


def paths(ins, site, limit=32):
    """Ways through the instructions before a call: a jump forward to a place
    before the call is followed, a conditional one is both taken and not."""
    index = {i.address: j for j, i in enumerate(ins)}
    index[site] = len(ins)
    out = []
    stack = [(0, [])]
    while stack and len(out) < limit:
        j, path = stack.pop()
        while j < len(ins):
            i = ins[j]
            t = num(i.op_str) if i.mnemonic.startswith("j") else None
            forward = t is not None and i.address < t <= site and t in index
            if i.mnemonic == "jmp":
                if not forward:
                    path = None
                    break
                j = index[t]
                continue
            if forward:
                stack.append((index[t], list(path)))
            path.append(i)
            j += 1
        if path is not None:
            out.append(path)
    return out


class Sites:
    def __init__(self, exe):
        self.exe = exe
        self.names = {}  # variable -> name
        self.rows = []

    def states(self, site):
        """The function of a call and the state before it on every way to it."""
        fn = self.exe.function(site)
        ins = self.exe.before(site, fn) if fn else []
        zero = self.exe.zero_registers(fn, ins[0].address) if ins else set()
        out = []
        for path in paths(ins, site) if ins else []:
            r = simulate(path, zero)
            if r:
                out.append(r)
        return fn, out

    def collect_names(self):
        exe = self.exe
        for target in (0x408240, 0x408980, 0x4089B0, 0x473EC0, INTERN):
            for site in exe.calls(target):
                fn, states = self.states(site)
                for _, st, sp in states:
                    if self.named(site, fn, target, st.get(sp), st.get(sp + 4)):
                        break

    def named(self, site, fn, target, var, name):
        exe = self.exe
        if not var or not name:
            return False
        if target == INTERN:
            if name[0] != "hash":
                return False
            name = name[1]
        s = None
        if name[0] == "imm":
            s = exe.cstr(name[1])
        elif name[0] == "glob":
            # A table of names in .data: a pointer to the string.
            o = exe.off(name[1])
            if o is not None:
                s = exe.cstr(struct.unpack_from("<I", exe.d, o)[0])
        if not s:
            return False
        if var[0] == "imm":
            old = self.names.setdefault(var[1], s)
            if old.lower() != s.lower():
                sys.exit("variable %#x is named `%s` and `%s`" % (var[1], old, s))
        if target in (0x408240, INTERN):
            self.rows.append(("name", site, fn, target, s, self.variable_of(var, False), ""))
        return True

    def message_of(self, v):
        """A value that is a hash (loaded from a variable) -> message column."""
        if v is None:
            return "?"
        if v[0] == "glob":
            return self.names.get(v[1], "var:0x%08X" % v[1])
        if v[0] == "mem":
            return "[obj+0x%X]" % v[2] if v[1][0] in ("reg", "addr") else "[obj]"
        if v[0] == "imm":
            return "hash:0x%08X" % v[1]
        return "?"

    def variable_of(self, v, named=True):
        """A value that is the address of a variable -> message column."""
        if v is None:
            return "?"
        if v[0] == "imm":
            return (named and self.names.get(v[1])) or "var:0x%08X" % v[1]
        if v[0] == "addr":
            return "[obj+0x%X]" % v[2]
        if v[0] == "reg":
            return "[obj+0x0]"
        return "?"

    def data_of(self, v, st):
        if v is None:
            return "?"
        if v[0] == "imm":
            return "none" if v[1] == 0 else "?"
        if v[0] == "lea":
            first = st.get(v[1])
            if first and first[0] == "ctor":
                first = self.exe.constructed(first[1])
            if first and first[0] == "imm" and 0xD55000 <= first[1] < 0xE50000:
                return "obj:0x%08X" % first[1]
            return "obj:?"
        if v[0] == "reg":
            return "reg"
        if v[0] == "mem":
            return "mem"
        return "?"

    @staticmethod
    def force_of(v):
        if v is None:
            return "?"
        if v[0] == "imm":
            return str(v[1] & 0xFF)
        return "?"

    def collect(self):
        exe = self.exe
        self.collect_names()
        for target, arg in LISTEN.items():
            if target == 0x408980:
                continue
            for site in exe.calls(target):
                fn, states = self.states(site)
                found = {self.variable_of(st.get(sp + 4 * arg)) for _, st, sp in states}
                self.add("listen", site, fn, target, {(m, "", "") for m in found})
        for site in exe.calls(SEND):
            fn, states = self.states(site)
            found = set()
            for _, st, sp in states:
                msg = data = "?"
                m = st.get(sp)
                force = self.force_of(st.get(sp + 4))
                if m and m[0] == "lea":
                    msg = self.message_of(st.get(m[1]))
                    data = self.data_of(st.get(m[1] + 4), st)
                found.add((msg, data, force))
            self.add("send", site, fn, SEND, found)
        for target, (kind, lay) in WRAPPERS.items():
            for site in exe.calls(target):
                fn, states = self.states(site)
                found = set()
                for _, st, sp in states:
                    msg = data = force = "?"
                    if "msg" in lay:
                        m = st.get(sp + 4 * lay["msg"])
                        if m and m[0] == "lea":
                            msg = self.message_of(st.get(m[1]))
                            data = self.data_of(st.get(m[1] + 4), st)
                    else:
                        msg = self.variable_of(st.get(sp + 4 * lay["var"]))
                        if lay.get("nodata"):
                            data = "none"
                        else:
                            data = self.data_of(st.get(sp + 4 * lay["data"]), st)
                    if "force" in lay:
                        force = self.force_of(st.get(sp + 4 * lay["force"]))
                    found.add((msg, data, force))
                self.add(kind, site, fn, target, found)
        order = {"name": 0, "listen": 1, "send": 2, "direct": 3, "delayed": 4}
        self.rows.sort(key=lambda r: (order[r[0]], r[1], r[4:]))

    def add(self, kind, site, fn, target, found):
        """One row per different reading of a call; a way the pass could not
        follow is dropped when another way names the message."""
        if any(f[0] != "?" for f in found):
            found = {f for f in found if f[0] != "?"}
        for msg, data, force in sorted(found) or [("?", "?" if kind != "listen" else "", "?" if kind != "listen" else "")]:
            self.rows.append((kind, site, fn, target, msg, data, force))

    def text(self):
        out = [
            "# Places of godfather2.exe (PC 1.0.789.0) that name a message, subscribe to it or",
            "# put it on the bus. Generated by research/bus_sites.py from the executable; do",
            "# not edit by hand. Columns (tab-separated): kind, site, function, call, message,",
            "# data, force. See docs/analysis/message-bus.md.",
        ]
        for kind, site, fn, target, msg, data, force in self.rows:
            out.append("%s\t0x%08X\t0x%08X\t0x%08X\t%s\t%s\t%s" % (kind, site, fn, target, msg, data, force))
        return "\n".join(out) + "\n"


def main():
    if len(sys.argv) < 2 or sys.argv[1] not in ("build", "check", "stats"):
        sys.exit(__doc__)
    game = os.environ.get("EARS_GF2_DIR")
    if not game:
        sys.exit("EARS_GF2_DIR is not set")
    sites = Sites(Exe(os.path.join(game, "godfather2.exe")))
    sites.collect()
    if sys.argv[1] == "build":
        sys.stdout.write(sites.text())
    elif sys.argv[1] == "check":
        if open(sys.argv[2]).read() != sites.text():
            sys.exit("%s is out of date" % sys.argv[2])
        # Every name the code gives to the bus is a message of the engine list.
        listed = os.path.join(os.path.dirname(os.path.abspath(sys.argv[2])), "engine_messages.txt")
        known = {l.strip().lower() for l in open(listed) if l.strip() and not l.startswith("#")}
        missing = sorted({r[4] for r in sites.rows if r[0] == "name" and r[4].lower() not in known})
        if missing:
            sys.exit("not in engine_messages.txt: %s" % ", ".join(missing))
        print("ok: %d rows" % len(sites.rows))
    else:
        c = collections.Counter()
        named = collections.defaultdict(set)
        for kind, _, _, target, msg, data, _ in sites.rows:
            c[kind] += 1
            c["%s via 0x%X" % (kind, target)] += 1
            if kind in ("send", "direct", "delayed"):
                what = "named" if not msg.startswith(("[", "var:", "hash:", "?")) else msg.split("+")[0].split(":")[0]
                c["%s message %s" % (kind, what)] += 1
                c["%s data %s" % (kind, data.split(":")[0])] += 1
                if what == "named":
                    named[msg].add(data)
        for k in sorted(c):
            print("%6d  %s" % (c[k], k))
        print("%6d  variables with a name" % len(sites.names))
        print("%6d  named messages with a sender in engine code" % len(named))
        print("%6d  of them always without data" % sum(1 for v in named.values() if v == {"none"}))
        print("%6d  of them with a data object at every site" % sum(1 for v in named.values() if all(x.startswith("obj:0x") for x in v)))


if __name__ == "__main__":
    main()

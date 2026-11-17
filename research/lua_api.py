#!/usr/bin/env python3
"""Lua functions and tables godfather2.exe (PC 1.0.789.0) registers for scripts.

    EARS_GF2_DIR=<game> python3 research/lua_api.py scan
        every registration found in the exe: owner, name, where it is
        registered, the C function
    EARS_GF2_DIR=<game> python3 research/lua_api.py check [data/gf2/lua_api.toml]
        the addresses of the data file against the exe, both ways
    python3 research/lua_api.py usage <dir of decompiled scripts>
        how often the game's own scripts call each function of the data file
        (`doctor-toolkit lua-decompile <game> -o <dir>` writes the scripts)

How a registration looks in the code (docs/analysis/lua-api.md §1):

1. inline `lua_register`: `push name; push L; call lua_pushstring` (0x625CA0),
   then a C closure is made in place (`mov [eax+0xC], fn`) and stored in the
   globals with luaV_settable (0x633D60);
2. the same with a call of lua_pushcclosure (0x625D70): `push 0; push fn;
   push L; call`;
3. arrays of `{name, fn}` pairs ended by `{0, 0}`: luaL_openlib (0x628260)
   for the standard libraries and the engine's own walker (0x4AEB70) for
   metatables and the `GroupManager` table.

The functions that take the registrations back (they store nil) have no
function pointer after the name and drop out by themselves.
"""
import os
import re
import struct
import sys

BASE = 0x400000
TEXT = (0x401000, 0xD54000)
DATA = (0xD55000, 0x112A000)
PUSHSTRING = 0x625CA0
PUSHCCLOSURE = 0x625D70

# Arrays of pairs: address -> owner. The owner of a metatable is its name in
# the Lua registry (luaL_newmetatable); `GroupManager` is a global table.
ARRAYS = {
    0xE2AA58: "_G", 0xE2AB28: "coroutine", 0xE2AA10: "table", 0xE2AB58: "string",
    0xE2A948: "math",
    0xE32148: "GroupManager", 0xE32180: "Group", 0xE32340: "Variable",
    0xE32518: "UInt32", 0xE325F8: "Int32", 0xE326DC: "Base", 0xE36AB0: "MessageReceiver",
}
# Inline registrations that do not go to the globals: address of the
# lua_pushstring call -> owner (the tolua library table and tolua's own
# metatables, which no script of the game touches).
OWNERS = {0x629F28: "tolua", 0x629F86: "tolua", 0x629FE2: "tolua", 0x62A03E: "tolua"}
SKIP = range(0x63B000, 0x63D000)  # tolua class metatables (__index, __add, ...)


def exe():
    game = os.environ.get("EARS_GF2_DIR")
    if not game:
        sys.exit("lua_api: set EARS_GF2_DIR to the game directory")
    data = open(os.path.join(game, "godfather2.exe"), "rb").read()
    if len(data) != 13840384:
        sys.exit("lua_api: godfather2.exe is not the 1.0.789.0 edition (size differs)")
    return data


def cstr(d, va):
    if not DATA[0] <= va < DATA[1]:
        return None
    o = va - BASE
    s = d[o:d.find(b"\0", o)]
    if 0 < len(s) < 80 and all(32 <= c < 127 for c in s):
        return s.decode()
    return None


def calls(d, target):
    out = []
    for m in re.finditer(b"\xE8", d[TEXT[0] - BASE:TEXT[1] - BASE]):
        o = m.start() + TEXT[0] - BASE
        if o + 5 + struct.unpack_from("<i", d, o + 1)[0] + BASE == target:
            out.append(o)
    return out


def scan(d):
    """[(owner, name, register address, function address)]"""
    found = []
    sites = calls(d, PUSHSTRING)
    for i, o in enumerate(sites):
        name = None
        for k in range(5, 24):
            if d[o - k] == 0x68:
                name = cstr(d, struct.unpack_from("<I", d, o - k + 1)[0])
                if name:
                    break
        nxt = sites[i + 1] if i + 1 < len(sites) else o + 200
        window = d[o + 5:o + 5 + min(nxt - o - 5, 120)]
        fn = None
        m = re.search(b"\xC7[\x40-\x47]\x0C(....)", window, re.S)
        if m:
            fn = struct.unpack("<I", m.group(1))[0]
        else:
            m = re.search(b"\x68(....)[\x50-\x57]\xE8(....)", window, re.S)
            if m:
                at = o + 5 + m.start() + 6
                if at + 5 + struct.unpack("<i", m.group(2))[0] + BASE == PUSHCCLOSURE:
                    fn = struct.unpack("<I", m.group(1))[0]
        va = o + BASE
        if not name or fn is None or not TEXT[0] <= fn < TEXT[1] or va in SKIP:
            continue
        found.append((OWNERS.get(va, ""), name, va, fn))
    for at, owner in ARRAYS.items():
        o = at - BASE
        while True:
            name_va, fn = struct.unpack_from("<II", d, o)
            if name_va == 0:
                break
            found.append((owner, cstr(d, name_va), o + BASE, fn))
            o += 8
    return found


def toml_functions(path):
    import tomllib
    return tomllib.load(open(path, "rb"))


def main():
    cmd = sys.argv[1] if len(sys.argv) > 1 else "scan"
    if cmd == "scan":
        for owner, name, reg, fn in scan(exe()):
            print(f"{owner}\t{name}\t0x{reg:08X}\t0x{fn:08X}")
    elif cmd == "check":
        path = sys.argv[2] if len(sys.argv) > 2 else "data/gf2/lua_api.toml"
        api = toml_functions(path)
        # The same name registered twice (EclipseLog: once per Lua state) is one entry
        # with the registrations listed.
        seen = {}
        for owner, name, reg, fn in scan(exe()):
            seen.setdefault((owner, name), []).append((reg, fn))
        bad = 0
        listed = set()
        for f in api["function"]:
            if f["kind"] == "lua":
                continue
            key = (f.get("owner", ""), f["name"])
            listed.add(key)
            want = sorted((int(r, 16), int(f["impl"], 16)) for r in f["register"])
            if sorted(seen.get(key, [])) != want:
                print("differs:", key, f["register"], f["impl"], seen.get(key))
                bad += 1
        for key in seen:
            if key not in listed:
                print("not in the data file:", key, seen[key])
                bad += 1
        print(f"{len(listed)} registered functions checked, {bad} problem(s)")
        sys.exit(1 if bad else 0)
    elif cmd == "usage":
        import glob
        api = toml_functions("data/gf2/lua_api.toml")
        text = {p: open(p).read() for p in glob.glob(os.path.join(sys.argv[2], "**", "*.lua"), recursive=True)}
        for f in api["function"]:
            if f.get("owner", "") in ("_G", "coroutine", "table", "string", "math", "tolua"):
                continue
            pat = re.compile(r"(?<![\w])(?:[\w.]+[:.])?" + re.escape(f["name"]) + r"\(")
            n = sum(len(pat.findall(t)) for t in text.values())
            files = sum(1 for t in text.values() if pat.search(t))
            print(f"{f.get('owner', '')}\t{f['name']}\t{n}\t{files}")
    else:
        sys.exit(__doc__)


if __name__ == "__main__":
    main()

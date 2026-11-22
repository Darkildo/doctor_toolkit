#!/usr/bin/env python3
"""Prints the table of places the platform module checks (stage 80):
`CHECKS` of crates/doctor-toolkit-live/src/platform.rs, from godfather2.exe
(PC 1.0.789.0).

Usage:
    EARS_GF2_DIR=<game dir> platform_checks.py          # the Rust table
    EARS_GF2_DIR=<game dir> platform_checks.py verify   # compare with platform.rs

Every place is a function the module calls, an instruction it patches or a
method table it copies; the length is chosen so that no byte belongs to a
place another module of the toolkit patches (the test
`the_platform_keeps_clear_of_the_other_modules` checks that).
"""
import os
import re
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
from exe_xrefs import Exe  # noqa: E402

# address, length, what
PLACES = [
    (0x004833A0, 16, "SimGroup class registration (cdecl: hash, name, factory, size)"),
    (0x00483410, 10, "object creation from a record: looks the class up in the registry 0x012239BC"),
    (0x004834F8, 12, "object creation: factory(record, extra), then the reader, method +4 of the object"),
    (0x0043B910, 16, "object memory (cdecl: size, extra) from the allocator of the SimManager"),
    (0x0046C590, 16, "Base constructor (thiscall: record)"),
    (0x0046C560, 16, "Base deleting destructor (thiscall: flags)"),
    (0x0043AFF0, 16, "attribute packet iterator: the layout of a record the module reads itself"),
    (0x00408260, 16, "message variable from a hash (cdecl: variable, hash)"),
    (0x00408310, 16, "release a message variable (cdecl: variable)"),
    (0x00408900, 16, "subscribe a listener (cdecl: listener, variable, priority)"),
    (0x004086D0, 16, "unsubscribe a listener (thiscall: variable)"),
    (0x00408A1A, 10, "message bus: `xor al, al; test esi, esi; je 0x00408AE4` (the filter hook replaces these)"),
    (0x00408A24, 15, "message bus: the search of the entry goes on at 0x00408A24, a miss returns at 0x00408A2F"),
    (0x00408AE4, 4, "message bus: `pop edi; pop esi; pop ecx; ret`, the end of a send"),
    (0x00408BF0, 16, "delivery to one listener (cdecl: message, listener, force)"),
    (0x00440590, 16, "delayed messages (thiscall: variable, listener, seconds, data, flags)"),
    (0x0045CA00, 9, "message data: constructor of the object pointer (thiscall: object)"),
    (0x0045CAC0, 16, "message data: destructor of the object pointer"),
    (0x009C8E50, 8, "operator new of the game (cdecl: size)"),
    (0x0068AB55, 5, "application start: call 0x00688AF0, the registration of the classes (the start hook replaces this call)"),
    (0x00681860, 5, "game object: call 0x0043E140, the constructor of the LuaManager (the Lua hook replaces this call)"),
    (0x0043E2E0, 16, "LuaManager: add a binder (thiscall: binder), registers it in both states"),
    (0x0043E34E, 16, "LuaManager: adding a binder calls its method +4 with the state +0x18"),
    (0x00625CA0, 16, "lua_pushstring (cdecl)"),
    (0x00625D70, 16, "lua_pushcclosure (cdecl)"),
    (0x00626030, 16, "lua_settable (cdecl)"),
    (0x006259B0, 16, "lua_tonumber (cdecl, float in ST0)"),
    (0x00625A30, 16, "lua_toboolean (cdecl)"),
    (0x00625A70, 16, "lua_tostring (cdecl)"),
    (0x00625C20, 16, "lua_pushnil (cdecl): top at L+8, values of 8 bytes"),
    (0x00625C40, 16, "lua_pushnumber (cdecl, a float)"),
    (0x00625DF0, 16, "lua_pushboolean (cdecl)"),
    (0x00625E10, 16, "lua_pushlightuserdata (cdecl)"),
    (0x00625B60, 16, "lua_touserdata: base at L+0xC, types 2 and 7"),
    (0x00E3280C, 28, "method table of Base: destructor, reader, five more"),
    (0x00E32828, 16, "listener method table of Base: destructor, handler, switch off, switch on"),
    (0x00E32838, 4, "third method table of Base: destructor"),
    # Stage 115: the frame tick and the device registry.
    (0x0068BC15, 5, "frame of both main loops: call 0x0040EBB0, the tick (the tick hook replaces this call)"),
    (0x0040EBB0, 16, "the tick (cdecl, no arguments): the tick mode 0x01205258, then the pre-tick, tick and post-tick messages"),
    (0x00607CA0, 16, "device registry: register (cdecl: on_lost, on_reset, u8 priority; 0xFF appends)"),
    (0x00607DD0, 16, "device registry: after Reset, on_reset(arg) of every entry, count at 0x012224A8"),
    (0x00607FCE, 16, "device registry: before Reset, on_lost(arg) of every entry from the last"),
    (0x00607FDE, 10, "device registry: the entries at 0x01222328, 12 bytes each"),
    # Stage 116: the overlay.
    (0x00608E9E, 35, "frame: `mov eax,[0x01205750]` before Present (the overlay hook replaces these 5 bytes), Present, the lost flag 0x0119DA5E"),
    # Stage 117: the input gate.
    (0x00B941E5, 11, "XInput poll: push the state and the port, call 0x00D37316 XInputGetState (the gate replaces this call)"),
    (0x00D37316, 6, "XInputGetState: jmp [0x00D55470], the import"),
    (0x00B9518C, 19, "keyboard: mov ecx,[edx+0x24]; add esp,8 (the gate replaces these 6 bytes), GetDeviceState(256, [esp+0x2C])"),
    (0x00B95216, 13, "mouse: GetDeviceState(0x14, [esp+0x18]): mov eax,[ecx+0x24]; call eax (the gate replaces these 5 bytes)"),
    (0x00B952B1, 16, "joystick: mov ecx,[edx+0x24]; lea eax,[esp+0x2C] (the gate replaces these 7 bytes), GetDeviceState(0x110)"),
    (0x0069E6A9, 18, "menu cursor: GetCursorPos call [0x00D55304] (the gate replaces these 6 bytes), then ScreenToClient call [0x00D55308]"),
    (0x0069EB00, 42, "menu cursor: the movement since the last call, 0x0112A04C/0x0112A050 as floats, then 0 (the gate replaces this function)"),
]


def table(exe):
    out = ["pub const CHECKS: &[Check] = &["]
    for va, n, what in PLACES:
        b = exe.d[exe.off(va):exe.off(va) + n]
        out.append("    Check {")
        out.append("        address: 0x%08X," % va)
        out.append('        what: "%s",' % what)
        out.append("        bytes: &[%s]," % ", ".join("0x%02X" % x for x in b))
        out.append("    },")
    out.append("];")
    return "\n".join(out)


def squash(text):
    return re.sub(r"\s+", "", text).replace(",]", "]").replace(",}", "}")


def main():
    exe = Exe()
    text = table(exe)
    if sys.argv[1:] == ["verify"]:
        path = os.path.join(HERE, "..", "crates", "doctor-toolkit-live", "src", "platform.rs")
        if squash(text) not in squash(open(path).read()):
            sys.exit("platform.rs: CHECKS differ from the executable; paste the output of this script")
        print("platform.rs: %d places match the executable" % len(PLACES))
    else:
        print(text)


if __name__ == "__main__":
    main()

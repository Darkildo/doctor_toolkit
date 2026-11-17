#!/usr/bin/env python3
"""Builds data/gf2/message_senders.tsv: messages of The Godfather II (PC
1.0.789.0) that are put on the bus by something other than a SimGroup object
or engine code — Lua scripts, cut-scenes and the score document.

Usage:
    doctor-toolkit dump --type lua -o <lua_dir> <game_dir>
    doctor-toolkit dump --type trinity_seq_master -o <seq_dir> <game_dir>
    doctor-toolkit dump --type score -o <score_dir> <game_dir>
    message_senders.py build <lua_dir> <seq_dir> <score_dir> > data/gf2/message_senders.tsv
    message_senders.py check <lua_dir> <seq_dir> <score_dir> data/gf2/message_senders.tsv
    message_senders.py stats <lua_dir> <seq_dir> <score_dir>

The inputs are the resources as `doctor-toolkit dump` writes them
(`<dir>/<stream path>/<index>_<type>_<id>.bin`); the game is never written.
Pure Python, no dependencies.

Only sends that the code of godfather2.exe proves are listed:

* `lua` — a call of the global `SendMessage` or `SendMessageEx` whose first
  argument is a string constant loaded in the same function (Lua 5.0
  byte-code of the `lua` resources; registers are followed along straight
  code and forgotten at every jump target). Both functions build a message
  from the name and call the bus `0x00408A00` (`SendMessage`: C function
  `0x004C8510` -> `0x004C82C0`; `SendMessageEx`: `0x004C86D0`). A call whose
  name is not a constant is counted by `stats`, not listed.
* `cutscene` — a text command `0x402` of kind 1 of a `trinity_seq_master`
  resource: the handler `0x00845A50` (branch `0x00845DB6`) names a message
  variable with the text (`0x00408240`) and calls `0x00408A00` with data 0.
* `score` — an action of type 1 of the `score` document: the interpreter
  `0x008948A0` names a message variable with the string of the action and
  puts it on the delayed queue `0x00440590` for every listener, with the
  delay of the action in seconds and data 0.

Output, tab-separated, sorted: name, kind, resource, detail, data.

- name: the message name as the sender spells it;
- kind: `lua`, `cutscene` or `score`;
- resource: the stream path, then `/` and the script file (`lua`), the name of
  the sequence (`cutscene`) or `score` (`score`);
- detail: `SendMessage:<line>` or `SendMessageEx:<line>` (line of the call in
  the script source), `command:<index>@<seconds>` (index of the command in
  the sequence and its time on the scale of the shot), `action:<count>@<delay>`
  (how many actions of the document send the name and their delay, seconds);
- data: what goes into the data word: `-` when nothing (0), `params` for the
  argument table of `SendMessageEx`, a constant for a constant third argument
  of `SendMessage`, `?` when the argument is not a constant.

`stats` also reads data/gf2/game_messages.tsv and data/gf2/engine_messages.txt
and tells how many catalogue messages without the other end got a sender.
"""
import collections
import os
import struct
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
DATA = os.path.join(HERE, '..', 'data', 'gf2')


def sdbm_lower(s):
    h = 0
    for c in s.lower().encode('latin1'):
        h = (c + h * 65599) & 0xFFFFFFFF
    return h


def resources(root):
    """Yields (stream path, bytes) of every dumped resource, sorted."""
    for dp, _, fns in sorted(os.walk(root)):
        stream = os.path.relpath(dp, root).replace(os.sep, '/')
        for fn in sorted(fns):
            yield stream, open(os.path.join(dp, fn), 'rb').read()


# --- Lua 5.0 chunks -----------------------------------------------------------------

LUA_HEADER = b'\x1bLua\x50\x01\x04\x04\x04\x06\x08\x09\x09\x04'
OP_MOVE, OP_LOADK, OP_GETGLOBAL = 0, 1, 5
OP_JMP, OP_EQ, OP_LT, OP_LE, OP_TEST, OP_CALL, OP_TAILCALL = 20, 21, 22, 23, 24, 25, 26
OP_FORLOOP, OP_TFORLOOP, OP_TFORPREP = 28, 29, 30
# Instructions that do not write register A.
NO_WRITE = {7, 8, 9, OP_JMP, OP_EQ, OP_LT, OP_LE, 27, 31, 32, 33}
SENDERS = ('SendMessage', 'SendMessageEx')


class Reader:
    def __init__(self, data, pos):
        self.d, self.p = data, pos

    def u8(self):
        self.p += 1
        return self.d[self.p - 1]

    def u32(self):
        self.p += 4
        return struct.unpack_from('<I', self.d, self.p - 4)[0]

    def string(self):
        n = self.u32()
        if n == 0:
            return None
        self.p += n
        return self.d[self.p - n:self.p - 1].decode('latin1')


def lua_function(r, source=None):
    """One function of a Lua 5.0 chunk (4-byte numbers) and its children."""
    f = {'source': r.string() or source}
    f['line'] = r.u32()
    r.p += 4  # nups, numparams, is_vararg, maxstacksize
    f['lines'] = [r.u32() for _ in range(r.u32())]
    for _ in range(r.u32()):  # locals
        r.string()
        r.p += 8
    for _ in range(r.u32()):  # upvalue names
        r.string()
    consts = []
    for _ in range(r.u32()):
        t = r.u8()
        if t == 4:
            consts.append(r.string())
        elif t == 3:
            consts.append(struct.unpack_from('<f', r.d, r.p)[0])
            r.p += 4
        elif t == 0:
            consts.append(None)
        else:
            raise ValueError('constant of type %d' % t)
    f['consts'] = consts
    f['protos'] = [lua_function(r, f['source']) for _ in range(r.u32())]
    f['code'] = [r.u32() for _ in range(r.u32())]
    return f


def lua_calls(f):
    """Yields (function name, line, arguments) for every call of a global in
    `f` and its children; an argument is ('k', constant) or None."""
    code = f['code']
    targets = set()
    for pc, ins in enumerate(code):
        op = ins & 0x3F
        if op in (OP_JMP, OP_FORLOOP, OP_TFORPREP):
            targets.add(pc + 1 + ((ins >> 6) & 0x3FFFF) - 131071)
        if op in (OP_EQ, OP_LT, OP_LE, OP_TEST, OP_TFORLOOP):
            targets.add(pc + 2)
    regs = {}
    for pc, ins in enumerate(code):
        if pc in targets:
            regs = {}
        op, a = ins & 0x3F, ins >> 24
        b, c, bx = (ins >> 15) & 0x1FF, (ins >> 6) & 0x1FF, (ins >> 6) & 0x3FFFF
        if op == OP_GETGLOBAL:
            regs[a] = ('g', f['consts'][bx])
        elif op == OP_LOADK:
            regs[a] = ('k', f['consts'][bx])
        elif op == OP_MOVE:
            regs[a] = regs.get(b)
        elif op in (OP_CALL, OP_TAILCALL):
            fn = regs.get(a)
            if fn and fn[0] == 'g':
                line = f['lines'][pc] if pc < len(f['lines']) else 0
                # B = 0: arguments up to the top of the stack (a call inside).
                args = None if b == 0 else [regs.get(a + 1 + i) for i in range(b - 1)]
                yield fn[1], line, args
            for k in [k for k in regs if k >= a]:
                del regs[k]
        elif op not in NO_WRITE:
            regs.pop(a, None)
            if op == 3:  # LOADNIL A..B
                for k in range(a, b + 1):
                    regs.pop(k, None)
            if op == 11:  # SELF writes A+1
                regs.pop(a + 1, None)
    for p in f['protos']:
        yield from lua_calls(p)


def lua_rows(root, unknown):
    for stream, d in resources(root):
        if not d.startswith(LUA_HEADER):
            raise ValueError('%s: not a Lua 5.0 chunk of the game' % stream)
        f = lua_function(Reader(d, len(LUA_HEADER) + 4))
        script = (f['source'] or '?').replace('\\', '/').rsplit('/', 1)[-1].lstrip('@')
        for name, line, args in lua_calls(f):
            if name not in SENDERS:
                continue
            first = args[0] if args else None
            if not first or first[0] != 'k' or not isinstance(first[1], str):
                unknown.append((stream, script, name, line))
                continue
            if name == 'SendMessageEx':
                data = 'params' if len(args) > 1 else '-'
            elif len(args) < 3:
                data = '-'
            else:
                third = args[2]
                data = repr(third[1]) if third and third[0] == 'k' else '?'
            yield first[1], 'lua', '%s/%s' % (stream, script), '%s:%d' % (name, line), data


# --- Cut-scenes ---------------------------------------------------------------------

def cutscene_rows(root):
    for stream, d in resources(root):
        if d[:4] != b'seqm':
            raise ValueError('%s: not a trinity_seq_master' % stream)
        for slot in range(256):
            blk = struct.unpack_from('<I', d, 0x120 + 4 * slot)[0]
            if blk == 0xBBBBBBBB:
                continue
            if d[blk:blk + 4] != b'seqb':
                raise ValueError('%s: bad block' % stream)
            seq = d[blk + 0x30:d.index(b'\0', blk + 0x30)].decode('latin1')
            o = blk + struct.unpack_from('<I', d, blk + 0xB8)[0]
            index, clock = 0, 0.0
            while True:
                code, size = struct.unpack_from('<HH', d, o)
                if code == 0x100:
                    break
                if code == 0x101:
                    clock += struct.unpack_from('<f', d, o + 4)[0]
                elif code == 0x402 and d[o + 4] == 1:
                    text = d[o + 6:d.index(b'\0', o + 6)].decode('latin1')
                    yield text, 'cutscene', '%s/%s' % (stream, seq), 'command:%d@%.3f' % (index, clock), '-'
                o += size
                index += 1


# --- Score --------------------------------------------------------------------------

def score_actions(d):
    """Yields (message name, delay) of every action of type 1 of a `score`
    document: header words 12 and 14 are the offsets of the actions and of the
    strings; an action is `u16 size, u8 type, u8 flags, body`, a list of
    actions ends with a zero word."""
    if struct.unpack_from('<I', d, 0)[0] != 10:
        raise ValueError('score: version is not 10')
    actions, strings = struct.unpack_from('<I', d, 0x30)[0], struct.unpack_from('<I', d, 0x38)[0]
    o = actions
    while o < strings:
        size, kind = struct.unpack_from('<HB', d, o)
        if size == 0:
            o += 4
            continue
        if kind == 1:
            at, delay = struct.unpack_from('<If', d, o + 4)
            s = strings + at
            yield d[s:d.index(b'\0', s)].decode('latin1'), delay
        o += size
    if o != strings:
        raise ValueError('score: the actions do not end at the strings')


def score_rows(root):
    for stream, d in resources(root):
        found = collections.Counter(score_actions(d))
        for (name, delay), count in sorted(found.items()):
            if name:
                yield name, 'score', '%s/score' % stream, 'action:%d@%g' % (count, delay), '-'


# --- Output -------------------------------------------------------------------------

def build(lua, seq, score):
    unknown = []
    rows = sorted(set(lua_rows(lua, unknown)) | set(cutscene_rows(seq)) | set(score_rows(score)),
                  key=lambda r: (r[0].lower(), r))
    out = [
        '# Messages put on the bus by Lua scripts, cut-scenes and the score document of',
        '# The Godfather II (PC 1.0.789.0). Generated by research/message_senders.py from',
        '# the game files; do not edit by hand.',
        '# Columns (tab-separated): name, kind, resource, detail, data.',
    ]
    out += ['\t'.join(r) for r in rows]
    return '\n'.join(out) + '\n', rows, unknown


def stats(rows, unknown):
    by_kind = collections.defaultdict(set)
    for name, kind, *_ in rows:
        by_kind[kind].add(sdbm_lower(name))
    every = set().union(*by_kind.values())
    print('rows: %d; messages: %d' % (len(rows), len(every)))
    for kind in ('lua', 'cutscene', 'score'):
        print('  %-8s %4d rows, %4d messages' % (
            kind, sum(1 for r in rows if r[1] == kind), len(by_kind[kind])))
    print('Lua calls with a name that is not a constant: %d' % len(unknown))
    for u in unknown:
        print('  %s/%s %s line %d' % u)
    engine = set()
    for line in open(os.path.join(DATA, 'engine_messages.txt')):
        line = line.strip()
        if line and not line.startswith('#'):
            engine.add(sdbm_lower(line))
    game = {}
    for line in open(os.path.join(DATA, 'game_messages.tsv')):
        if not line.startswith('#'):
            f = line.rstrip('\n').split('\t')
            game[sdbm_lower(f[0])] = (f[1] != '-', f[2] != '-')
    heard = {h for h, (s, l) in game.items() if l and not s and h not in engine}
    sent = {h for h, (s, l) in game.items() if s and not l and h not in engine}
    print('catalogue: %d messages; heard, never sent: %d; sent, never heard: %d' % (
        len(game), len(heard), len(sent)))
    print('heard, never sent, with a sender here: %d' % len(heard & every))
    for kind in ('lua', 'cutscene', 'score'):
        print('  %-8s %d' % (kind, len(heard & by_kind[kind])))
    print('sent, never heard, also sent here: %d' % len(sent & every))
    print('senders here that no SimGroup uses: %d (of them named by the executable: %d)' % (
        len(every - set(game)), len((every - set(game)) & engine)))
    print('engine messages sent here: %d' % len(every & engine))


def main():
    if len(sys.argv) < 5 or sys.argv[1] not in ('build', 'check', 'stats'):
        sys.exit(__doc__)
    text, rows, unknown = build(*sys.argv[2:5])
    if sys.argv[1] == 'build':
        sys.stdout.write(text)
    elif sys.argv[1] == 'check':
        if open(sys.argv[5]).read() != text:
            sys.exit('%s is out of date' % sys.argv[5])
        print('ok: %d rows' % len(rows))
    else:
        stats(rows, unknown)


if __name__ == '__main__':
    main()

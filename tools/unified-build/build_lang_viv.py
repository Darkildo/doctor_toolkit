"""Collect the speech of one language from the archives of a disc into a new BIGF archive.
usage: build_lang_viv.py <out.viv> <lang> <cmd printing archive 0> <cmd 1> <cmd 2>
Each command writes a whole GODFATHn.viv to stdout (e.g. `7z x -so disc.iso GODFATH0.viv` or `cat file`)."""
import sys, struct, subprocess, os, shlex
out, lang = sys.argv[1], sys.argv[2]
ALIGN = 0x800
prefix = f'audiostreams\\{lang}\\'.encode()
entries = []   # (name bytes, size, tmp offset)
tmp = open(out + '.data', 'wb')
for cmd in sys.argv[3:]:
    p = subprocess.Popen(cmd, shell=True, stdout=subprocess.PIPE, bufsize=1 << 20)
    r = p.stdout
    h = r.read(16); assert h[:4] == b'BIGF', (cmd, h)
    n, hs = struct.unpack('>II', h[8:16]); body = r.read(hs - 16); pos = hs
    want = []; q = 0
    for _ in range(n):
        off, sz = struct.unpack('>II', body[q:q + 8]); q += 8
        e = body.index(b'\0', q); name = body[q:e]; q = e + 1
        if name.lower().startswith(prefix): want.append((off, sz, name))
    for off, sz, name in sorted(want):
        skip = off - pos
        assert skip >= 0, (name, off, pos)
        while skip:
            c = r.read(min(skip, 1 << 20)); assert c; skip -= len(c)
        d = r.read(sz); assert len(d) == sz, name
        pos = off + sz
        entries.append((name, sz, tmp.tell())); tmp.write(d)
    p.kill(); p.wait()
    print(cmd[:60], '->', len(want), 'files', file=sys.stderr)
tmp.close()
TRAILER = b'L283\x15\x05\0\0'   # ends the header of every archive of the game
hs = 16 + sum(8 + len(n) + 1 for n, _, _ in entries) + len(TRAILER)
first = (hs + ALIGN - 1) // ALIGN * ALIGN
offs = []; o = first
for name, sz, _ in entries:
    offs.append(o); o = (o + sz + ALIGN - 1) // ALIGN * ALIGN
total = offs[-1] + entries[-1][1]
with open(out, 'wb') as f, open(out + '.data', 'rb') as t:
    f.write(b'BIGF' + struct.pack('<I', total) + struct.pack('>II', len(entries), hs))
    for (name, sz, _), off in zip(entries, offs):
        f.write(struct.pack('>II', off, sz) + name + b'\0')
    f.write(TRAILER)
    for (name, sz, toff), off in zip(entries, offs):
        f.write(b'\0' * (off - f.tell())); t.seek(toff); f.write(t.read(sz))
os.remove(out + '.data')
print(out, 'files', len(entries), 'size', total, file=sys.stderr)

"""Direct3D 9 shader bytecode (SM 2/3) in godfather2.exe: find and disassemble.

The executable embeds 943 compiled shaders (808 pixel, 135 vertex) with
their constant tables (CTAB), which name every parameter and sampler.

    python3 research/d3d9_shaders.py GAME/godfather2.exe              # count
    python3 research/d3d9_shaders.py GAME/godfather2.exe find g_TintColor0 g_DetailMaskSampler
    python3 research/d3d9_shaders.py GAME/godfather2.exe dis ac07e8   # file offset, hex

Used for docs/formats/characters.md §2 (stage 17).
"""
import struct, sys

EXE = 'godfather2.exe'

OPS = {0: 'nop', 1: 'mov', 2: 'add', 3: 'sub', 4: 'mad', 5: 'mul', 6: 'rcp', 7: 'rsq', 8: 'dp3', 9: 'dp4',
       10: 'min', 11: 'max', 12: 'slt', 13: 'sge', 14: 'exp', 15: 'log', 16: 'lit', 17: 'dst', 18: 'lrp',
       19: 'frc', 20: 'm4x4', 21: 'm4x3', 22: 'm3x4', 23: 'm3x3', 24: 'm3x2', 25: 'call', 26: 'callnz',
       27: 'loop', 28: 'ret', 29: 'endloop', 30: 'label', 31: 'dcl', 32: 'pow', 33: 'crs', 34: 'sgn',
       35: 'abs', 36: 'nrm', 37: 'sincos', 38: 'rep', 39: 'endrep', 40: 'if', 41: 'ifc', 42: 'else',
       43: 'endif', 44: 'break', 45: 'breakc', 46: 'mova', 47: 'defb', 48: 'defi', 64: 'texcoord',
       65: 'texkill', 66: 'texld', 78: 'expp', 79: 'logp', 80: 'cnd', 81: 'def', 88: 'cmp', 89: 'bem',
       90: 'dp2add', 91: 'dsx', 92: 'dsy', 93: 'texldd', 94: 'setp', 95: 'texldl', 96: 'breakp'}
NOSRC = {'nop', 'else', 'endif', 'endloop', 'endrep', 'ret', 'break'}
NDST = {'texkill': 0}
RT = {0: 'r', 1: 'v', 2: 'c', 3: 't', 4: 'oPos', 5: 'oD', 6: 'o', 7: 'i', 8: 'oC', 9: 'oDepth', 10: 's',
      11: 'c', 12: 'c', 13: 'c', 14: 'b', 15: 'aL', 16: 'h', 17: 'vMisc', 18: 'l', 19: 'p'}
USAGE = {0: 'position', 1: 'blendweight', 2: 'blendindices', 3: 'normal', 4: 'psize', 5: 'texcoord',
         6: 'tangent', 7: 'binormal', 8: 'tessfactor', 9: 'positiont', 10: 'color', 11: 'fog', 12: 'depth',
         13: 'sample'}
SAMP = {1: '1d', 2: '2d', 3: 'cube', 4: 'volume'}
SMOD = ['{}', '-{}', '{}_bias', '-{}_bias', '{}_bx2', '-{}_bx2', '1-{}', '{}_x2', '-{}_x2', '{}_dz', '{}_dw',
        'abs({})', '-abs({})', '!{}']
CMP = {1: 'gt', 2: 'eq', 3: 'ge', 4: 'lt', 5: 'ne', 6: 'le'}


def rtype(t):
    return ((t >> 28) & 7) | ((t >> 8) & 0x18)


class Shader:
    def __init__(self, data, at=0):
        self.data = data
        self.at = at
        self.version = struct.unpack_from('<I', data, at)[0]
        self.ps = (self.version >> 16) == 0xFFFF
        self.major = (self.version >> 8) & 0xFF
        self.tokens = []
        p = at + 4
        self.ctab = {}
        self.comments = []
        while True:
            t = struct.unpack_from('<I', data, p)[0]
            if t == 0xFFFF:
                p += 4
                break
            if (t & 0xFFFF) == 0xFFFE:
                n = (t >> 16) & 0x7FFF
                c = data[p + 4:p + 4 + 4 * n]
                self.comments.append(c)
                if c[:4] == b'CTAB':
                    self.parse_ctab(c[4:])
                p += 4 + 4 * n
                continue
            n = (t >> 24) & 0xF
            self.tokens.append((t, list(struct.unpack_from('<%dI' % n, data, p + 4))))
            p += 4 + 4 * n
        self.size = p - at
        self.consts = {}
        for name, (rs, ri, rc, default) in self.ctab.items():
            for k in range(rc):
                self.consts[('csb'[rs - 1] if rs else 'b', ri + k)] = name + ('[%d]' % k if rc > 1 else '')

    def parse_ctab(self, c):
        def cstr(o):
            return c[o:c.index(b'\0', o)].decode('latin1')
        size, creator, version, n, info, flags, target = struct.unpack_from('<7I', c, 0)
        self.target = cstr(target)
        for k in range(n):
            no, rs, ri, rc, _, ti, dv = struct.unpack_from('<IHHHHII', c, info + 20 * k)
            set_ = {0: 0, 1: 1, 2: 2, 3: 3}[rs]
            letter = {0: 'b', 1: 'i', 2: 'c', 3: 's'}[rs]
            default = None
            if dv and rs == 2:
                default = struct.unpack_from('<%df' % (4 * rc), c, dv)
            self.ctab[cstr(no)] = (rs, ri, rc, default)
        self.cmap = {}
        for name, (rs, ri, rc, d) in self.ctab.items():
            letter = {0: 'b', 1: 'i', 2: 'c', 3: 's'}[rs]
            for k in range(rc):
                self.cmap[(letter, ri + k)] = name + ('[%d]' % k if rc > 1 else '')

    def reg(self, t, rel=None):
        ty = rtype(t)
        n = t & 0x7FF
        name = RT.get(ty, '?%d' % ty)
        if ty == 17:
            s = ['vPos', 'vFace'][n] if n < 2 else 'vMisc%d' % n
        elif ty == 4:
            s = ['oPos', 'oFog', 'oPts'][n]
        else:
            s = '%s%d' % (name, n)
        letter = {2: 'c', 7: 'i', 14: 'b', 10: 's'}.get(ty)
        cm = getattr(self, 'cmap', {})
        if letter and (letter, n) in cm:
            s += '<%s>' % cm[(letter, n)]
        if rel is not None:
            s += '[%s]' % rel
        return s

    def dst(self, t):
        s = self.reg(t)
        m = (t >> 16) & 0xF
        if m != 0xF:
            s += '.' + ''.join('xyzw'[i] for i in range(4) if m >> i & 1)
        return s

    def src(self, t, extra=None):
        rel = None
        if t & (1 << 13) and extra is not None:
            rel = self.reg(extra) + '.' + 'xyzw'[(extra >> 16) & 3]
        s = self.reg(t, rel)
        sw = (t >> 16) & 0xFF
        comps = [sw >> (2 * i) & 3 for i in range(4)]
        if comps != [0, 1, 2, 3]:
            if len(set(comps)) == 1:
                s += '.' + 'xyzw'[comps[0]]
            else:
                s += '.' + ''.join('xyzw'[c] for c in comps)
        mod = (t >> 24) & 0xF
        return SMOD[mod].format(s) if mod < len(SMOD) else s

    def disasm(self):
        out = []
        ind = 0
        for t, params in self.tokens:
            op = t & 0xFFFF
            ctl = (t >> 16) & 0xFF
            name = OPS.get(op, 'op%d' % op)
            if name in ('endif', 'endloop', 'endrep', 'else'):
                ind -= 1
            pre = '  ' * max(ind, 0)
            if name == 'dcl':
                u, d = params[0], params[1]
                if rtype(d) == 10:
                    out.append(pre + 'dcl_%s %s' % (SAMP.get((u >> 27) & 0xF, '?'), self.dst(d)))
                elif rtype(d) in (1, 6, 3) and (u & 0x80000000):
                    out.append(pre + 'dcl_%s%d %s' % (USAGE.get(u & 0x1F, '?'), (u >> 16) & 0xF, self.dst(d)))
                else:
                    out.append(pre + 'dcl %s' % self.dst(d))
            elif name == 'def':
                out.append(pre + 'def %s, %s' % (self.reg(params[0]), ', '.join(
                    '%g' % struct.unpack('<f', struct.pack('<I', x))[0] for x in params[1:5])))
            elif name in ('defi',):
                out.append(pre + 'defi %s, %s' % (self.reg(params[0]), params[1:5]))
            elif name == 'defb':
                out.append(pre + 'defb %s, %s' % (self.reg(params[0]), params[1]))
            else:
                # Walk parameters: first dst (if any), then sources (each may carry a rel token).
                ops = []
                i = 0
                has_dst = name not in ('if', 'ifc', 'rep', 'loop', 'call', 'callnz', 'break', 'breakc',
                                       'label', 'texkill', 'else', 'endif', 'endloop', 'endrep', 'ret',
                                       'breakp') and name not in NOSRC
                if t & (1 << 28):
                    pass
                while i < len(params):
                    p = params[i]
                    extra = None
                    if p & (1 << 13) and i + 1 < len(params) and self.major >= 2 and not (has_dst and not ops):
                        extra = params[i + 1]
                    if has_dst and not ops:
                        s = self.dst(p)
                        rm = (p >> 20) & 0xF
                        if rm & 1:
                            name2 = '_sat'
                        else:
                            name2 = ''
                        if rm & 2:
                            name2 += '_pp'
                        ops.append(('D', s, name2))
                        i += 1
                        continue
                    ops.append(('S', self.src(p, extra), ''))
                    i += 2 if extra is not None else 1
                mods = ''.join(o[2] for o in ops if o[0] == 'D')
                if name in ('ifc', 'breakc', 'setp'):
                    name += '_' + CMP.get(ctl, str(ctl))
                if name == 'texld' and ctl == 1:
                    name = 'texldp'
                if name == 'texld' and ctl == 2:
                    name = 'texldb'
                pred = ''
                out.append(pre + name + mods + ' ' + ', '.join(o[1] for o in ops))
            if name in ('if', 'loop', 'rep', 'else') or name.startswith('ifc'):
                ind += 1
        return out


def all_shaders(data=None):
    if data is None:
        data = open(EXE, 'rb').read()
    out = []
    for tok in (0xFFFF0300, 0xFFFE0300, 0xFFFF0200, 0xFFFF0201, 0xFFFE0200):
        b = struct.pack('<I', tok)
        i = data.find(b)
        while i >= 0:
            if i % 4 == 0:
                nxt = struct.unpack_from('<I', data, i + 4)[0]
                if (nxt & 0xFFFF) == 0xFFFE:
                    try:
                        s = Shader(data, i)
                        if s.ctab:
                            out.append(s)
                    except Exception:
                        pass
            i = data.find(b, i + 1)
    return out


if __name__ == '__main__':
    exe = sys.argv[1]
    args = sys.argv[2:]
    data = open(exe, 'rb').read()
    sh = all_shaders(data)
    if args and args[0] == 'find':
        names = args[1:]
        for s in sh:
            if all(n in s.ctab for n in names):
                print('%s at 0x%x size %d consts %s' % ('ps' if s.ps else 'vs', s.at, s.size, sorted(s.ctab)))
    elif args and args[0] == 'dis':
        s = Shader(data, int(args[1], 16))
        for k, v in sorted(s.ctab.items(), key=lambda x: (x[1][0], x[1][1])):
            print('// %-32s %s%d x%d %s' % (k, 'bics'[v[0]], v[1], v[2], v[3] or ''))
        print('\n'.join(s.disasm()))
    else:
        print(len(sh), 'shaders,', sum(s.ps for s in sh), 'pixel')

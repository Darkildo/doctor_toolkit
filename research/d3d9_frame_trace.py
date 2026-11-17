"""One frame of Direct3D 9 calls written by the graphics module (stage 101).

`doctor_render.asi` with `draw_trace=1` writes every call of one frame the
game makes on its device into `doctor_render_trace.bin` when a file
`doctor_render_trace.go` appears next to it (docs/analysis/renderer.md §11).
This script reads such a file and prints where the draw calls of the frame
come from: passes (render targets), shaders, the game's code that issued
them, what changes between two draws, calls that set what is already set,
and how many draws could be merged or instanced.

    python3 research/d3d9_frame_trace.py TRACE.bin [--exe GAME/godfather2.exe]
        [--passes] [--shaders] [--callers] [--runs] [--draws PASS] [--json]

With --exe the shaders are matched to their offsets in the executable.
The trace is not game data, but it holds shader bytecode of the game: it
stays out of the repository.
"""
import collections
import hashlib
import json
import struct
import sys

sys.path.insert(0, __file__.rsplit('/', 1)[0])
import d3d9_shaders  # noqa: E402

MAGIC = b'GF2DRAW1'
FRAME, SHADER, DECL, VB, IB, SURFACE, TEXTURE, TABLE, CALLERS, FRAME_END, DROPPED, MERGED = range(0x100, 0x10C)
NAMES = {16: 'Reset', 34: 'StretchRect', 37: 'SetRenderTarget', 39: 'SetDepthStencilSurface', 41: 'BeginScene',
         42: 'EndScene', 43: 'Clear', 47: 'SetViewport', 57: 'SetRenderState', 65: 'SetTexture',
         69: 'SetSamplerState', 75: 'SetScissorRect', 81: 'DrawPrimitive', 82: 'DrawIndexedPrimitive',
         83: 'DrawPrimitiveUP', 84: 'DrawIndexedPrimitiveUP', 87: 'SetVertexDeclaration', 89: 'SetFVF',
         92: 'SetVertexShader', 94: 'SetVertexShaderConstantF', 100: 'SetStreamSource',
         102: 'SetStreamSourceFreq', 104: 'SetIndices', 107: 'SetPixelShader', 109: 'SetPixelShaderConstantF'}
DRAWS = (81, 82, 83, 84)
FORMATS = {20: 'R8G8B8', 21: 'A8R8G8B8', 22: 'X8R8G8B8', 23: 'R5G6B5', 28: 'A8', 32: 'A8B8G8R8', 36: 'A16B16G16R16',
           50: 'L8', 51: 'A8L8', 75: 'D24S8', 77: 'D24X8', 80: 'D16', 111: 'R16F', 112: 'G16R16F',
           113: 'A16B16G16R16F', 114: 'R32F', 115: 'G32R32F', 116: 'A32B32G32R32F',
           0x31545844: 'DXT1', 0x33545844: 'DXT3', 0x35545844: 'DXT5', 0x5A544E49: 'INTZ', 0x34324644: 'DF24',
           0x36314644: 'DF16', 0x5A574152: 'RAWZ', 0x4C4C554E: 'NULL'}
# Always-equal states are not interesting when two draws are compared.
SAME_FLAG = {57: 2, 65: 2, 69: 3, 87: 1, 92: 1, 107: 1, 94: 2, 109: 2, 100: 4, 104: 1}


def records(data):
    if data[:8] != MAGIC:
        raise SystemExit('not a frame trace')
    words = memoryview(data)[8:].cast('I')
    p = 0
    while p + 2 <= len(words):
        op, n = words[p], words[p + 1]
        yield op, words[p + 2:p + 2 + n]
        p += 2 + n


class Frame:
    def __init__(self, data, exe=None):
        self.shaders = {}      # pointer -> (kind, sha1, Shader)
        self.decls, self.buffers, self.surfaces, self.textures = {}, {}, {}, {}
        self.table = None
        self.calls = collections.Counter()
        self.same = collections.Counter()
        self.dropped = collections.Counter()
        self.draws = []
        self.passes = []
        self.merged = []       # (draws joined, primitive type, first index, primitives)
        exe_index = {}
        if exe:
            raw = open(exe, 'rb').read()
            for s in d3d9_shaders.all_shaders(raw):
                exe_index[hashlib.sha1(raw[s.at:s.at + s.size]).hexdigest()] = s.at
        st = {'rt': {}, 'ds': 0, 'vs': 0, 'ps': 0, 'decl': 0, 'ib': 0, 'streams': {}, 'tex': {}, 'vsc': {}, 'psc': {},
              'rs': {}, 'ss': {}, 'viewport': None, 'scissor': None}
        cur = None
        callers = ()
        changed = set()
        calls_since = collections.Counter()
        dropped_next = False
        for op, w in records(data):
            if op == FRAME:
                self.number = w[0]
            elif op == TABLE:
                self.table = bytes(w)
            elif op == SHADER:
                code = bytes(w[1:])
                try:
                    sh = d3d9_shaders.Shader(code, 0)
                    code = code[:sh.size]
                except Exception:
                    sh = None
                h = hashlib.sha1(code).hexdigest()
                self.shaders[w[0]] = {'ps': bool(sh and sh.ps), 'sha1': h, 'shader': sh, 'exe': exe_index.get(h),
                                      'size': len(code)}
            elif op == DECL:
                els = []
                for i in range(1, len(w) - 1, 2):
                    stream, offset = w[i] & 0xFFFF, w[i] >> 16
                    kind, method, usage, index = (w[i + 1] >> (8 * k) & 0xFF for k in range(4))
                    if stream == 0xFF:
                        break
                    els.append((stream, offset, kind, usage, index))
                self.decls[w[0]] = els
            elif op in (VB, IB):
                self.buffers[w[0]] = {'kind': 'vb' if op == VB else 'ib', 'format': w[1], 'usage': w[3], 'pool': w[4],
                                      'size': w[5]}
            elif op == SURFACE:
                self.surfaces[w[0]] = {'format': w[1], 'usage': w[3], 'pool': w[4], 'w': w[7], 'h': w[8]}
            elif op == TEXTURE:
                self.textures[w[0]] = {'type': w[1], 'levels': w[2], 'format': w[3], 'usage': w[5], 'pool': w[6],
                                       'w': w[9], 'h': w[10]}
            elif op == CALLERS:
                callers = tuple(w)
            elif op == DROPPED:
                dropped_next = True
            elif op == MERGED:
                self.merged.append(tuple(w))
            elif op == FRAME_END:
                pass
            else:
                self.calls[op] += 1
                calls_since[op] += 1
                if dropped_next:
                    self.dropped[op] += 1
                    dropped_next = False
                if op in SAME_FLAG and w[SAME_FLAG[op]]:
                    self.same[op] += 1
                if op == 37:
                    st['rt'][w[0]] = w[1]
                    if w[0] == 0:
                        cur = None
                elif op == 39:
                    st['ds'] = w[0]
                    cur = None
                elif op == 43:
                    if cur is None:
                        cur = self.new_pass(st)
                    cur['clears'].append((w[1], w[2]))
                elif op == 47:
                    st['viewport'] = tuple(w[:4])
                elif op == 75:
                    st['scissor'] = tuple(w)
                elif op == 57:
                    if st['rs'].get(w[0]) != w[1]:
                        changed.add('rs')
                        changed.add('rs%d' % w[0])
                    st['rs'][w[0]] = w[1]
                elif op == 65:
                    if st['tex'].get(w[0], 0) != w[1]:
                        changed.add('tex')
                    st['tex'][w[0]] = w[1]
                elif op == 69:
                    if st['ss'].get((w[0], w[1])) != w[2]:
                        changed.add('ss')
                    st['ss'][(w[0], w[1])] = w[2]
                elif op == 87:
                    if st['decl'] != w[0]:
                        changed.add('decl')
                    st['decl'] = w[0]
                elif op == 92:
                    if st['vs'] != w[0]:
                        changed.add('vs')
                    st['vs'] = w[0]
                elif op == 107:
                    if st['ps'] != w[0]:
                        changed.add('ps')
                    st['ps'] = w[0]
                elif op in (94, 109):
                    key = 'vsc' if op == 94 else 'psc'
                    start, count = w[0], w[1]
                    for k in range(count):
                        v = tuple(w[3 + 4 * k:7 + 4 * k])
                        if st[key].get(start + k) != v:
                            changed.add(key)
                            changed.add('%s%d' % (key, start + k))
                        st[key][start + k] = v
                elif op == 100:
                    v = (w[1], w[2], w[3])
                    if st['streams'].get(w[0]) != v:
                        changed.add('vb')
                    st['streams'][w[0]] = v
                elif op == 104:
                    if st['ib'] != w[0]:
                        changed.add('ib')
                    st['ib'] = w[0]
                elif op in DRAWS:
                    if cur is None:
                        cur = self.new_pass(st)
                    if op == 82:
                        prims, args = w[5], tuple(w)
                    elif op == 81:
                        prims, args = w[2], tuple(w)
                    else:
                        prims, args = (w[1] if op == 83 else w[3]), tuple(w)
                    vs_used = self.used_constants(st['vs'])
                    ps_used = self.used_constants(st['ps'])
                    d = {'op': op, 'args': args, 'prims': prims, 'callers': callers, 'pass': len(self.passes) - 1,
                         'vs': st['vs'], 'ps': st['ps'], 'decl': st['decl'], 'ib': st['ib'],
                         'streams': tuple(sorted(st['streams'].items())),
                         'tex': tuple(sorted((k, v) for k, v in st['tex'].items() if v)),
                         'changed': frozenset(changed), 'calls': dict(calls_since),
                         'vsc': tuple(st['vsc'].get(r) for r in vs_used), 'vs_used': vs_used,
                         'psc': tuple(st['psc'].get(r) for r in ps_used),
                         'rs': tuple(sorted(st['rs'].items())), 'viewport': st['viewport']}
                    self.draws.append(d)
                    cur['draws'].append(d)
                    changed = set()
                    calls_since = collections.Counter()
                    callers = ()

    def new_pass(self, st):
        p = {'rt': dict(st['rt']), 'ds': st['ds'], 'draws': [], 'clears': [], 'viewport': st['viewport']}
        self.passes.append(p)
        return p

    def used_constants(self, shader):
        info = self.shaders.get(shader)
        if not info or not info['shader']:
            return ()
        if 'used' not in info:
            sh = info['shader']
            regs = set()
            for name, (rs, ri, rc, _) in sh.ctab.items():
                if rs == 2:
                    regs.update(range(ri, ri + rc))
            info['used'] = tuple(sorted(regs))
        return info['used']

    def shader_name(self, pointer):
        info = self.shaders.get(pointer)
        if not info:
            return 'none' if not pointer else '%08x' % pointer
        tag = ('exe+%06x' % info['exe']) if info['exe'] is not None else info['sha1'][:10]
        return '%s %s' % ('ps' if info['ps'] else 'vs', tag)

    def shader_constants(self, pointer):
        info = self.shaders.get(pointer)
        if not info or not info['shader']:
            return []
        return sorted(info['shader'].ctab)

    def surface_name(self, pointer):
        s = self.surfaces.get(pointer)
        if not pointer:
            return '-'
        if not s:
            return '%08x' % pointer
        return '%dx%d %s' % (s['w'], s['h'], FORMATS.get(s['format'], '%#x' % s['format']))


def mesh_key(d):
    """What is drawn: buffers and the range of the draw."""
    return (d['op'], d['args'], d['decl'], d['ib'], d['streams'])


def material_key(d):
    return (d['vs'], d['ps'], d['tex'], d['psc'], d['rs'])


def report(f, opts):
    total = len(f.draws)
    prims = sum(d['prims'] for d in f.draws)
    ncalls = sum(f.calls.values())
    out = {'frame': f.number, 'draws': total, 'primitives': prims, 'calls': ncalls, 'passes': len(f.passes)}
    print('frame %d: %d draws, %d primitives, %d calls of the device, %d passes (changes of target)' % (
        f.number, total, prims, ncalls, len(f.passes)))
    print('\ncalls by method (count, of them setting what is already set):')
    out['methods'] = {}
    for op, n in f.calls.most_common():
        same = f.same.get(op, 0)
        out['methods'][NAMES.get(op, str(op))] = {'calls': n, 'same': same, 'dropped': f.dropped.get(op, 0)}
        print('  %-26s %7d %7s %s' % (NAMES.get(op, op), n, same if op in SAME_FLAG else '',
                                      ('dropped %d' % f.dropped[op]) if f.dropped.get(op) else ''))
    state_calls = sum(n for op, n in f.calls.items() if op in SAME_FLAG)
    same_calls = sum(f.same.values())
    out['state_calls'], out['same_calls'] = state_calls, same_calls
    print('  state calls %d, of them redundant %d (%.1f %%)' % (state_calls, same_calls,
                                                                100.0 * same_calls / max(state_calls, 1)))
    if f.merged:
        joined = sum(m[0] for m in f.merged)
        out['module'] = {'joined': joined, 'into': len(f.merged), 'to_device': total - joined + len(f.merged),
                         'indices': sum(m[3] + 2 for m in f.merged)}
        print('\nthe module joined %d draws into %d: %d draws reached the device (%.1f %% fewer)' % (
            joined, len(f.merged), total - joined + len(f.merged),
            100.0 * (joined - len(f.merged)) / max(total, 1)))
    kinds = collections.Counter(d['op'] for d in f.draws)
    print('\ndraws by kind: ' + ', '.join('%s %d' % (NAMES[k], n) for k, n in kinds.most_common()))
    sizes = collections.Counter()
    for d in f.draws:
        p = d['prims']
        sizes['<=2' if p <= 2 else '<=16' if p <= 16 else '<=64' if p <= 64 else '<=256' if p <= 256
              else '<=1024' if p <= 1024 else '>1024'] += 1
    print('draws by primitives: ' + ', '.join('%s: %d' % (k, sizes[k]) for k in
                                              ('<=2', '<=16', '<=64', '<=256', '<=1024', '>1024')))
    out['sizes'] = dict(sizes)

    # Passes, grouped by target.
    print('\npasses in order (target 0 | depth | draws | primitives | vs | ps | first callers):')
    out['pass_list'] = []
    groups = collections.OrderedDict()
    for i, p in enumerate(f.passes):
        if not p['draws'] and not opts.get('passes'):
            continue
        rt = f.surface_name(p['rt'].get(0, 0))
        extra = ''.join(' +' + f.surface_name(v) for k, v in sorted(p['rt'].items()) if k and v)
        ds = f.surface_name(p['ds'])
        n = len(p['draws'])
        pr = sum(d['prims'] for d in p['draws'])
        vs = len({d['vs'] for d in p['draws']})
        ps = len({d['ps'] for d in p['draws']})
        top = collections.Counter(d['callers'][:3] for d in p['draws']).most_common(1)
        chain = ' '.join('%06x' % a for a in top[0][0]) if top else ''
        rec = {'index': i, 'target': rt + extra, 'target_ptr': p['rt'].get(0, 0), 'depth': ds, 'draws': n,
               'primitives': pr, 'vs': vs, 'ps': ps, 'callers': chain, 'clears': len(p['clears'])}
        out['pass_list'].append(rec)
        if n >= opts.get('min_pass', 20) or opts.get('passes'):
            print('  %3d  %-22s %-16s %5d %8d %3d %3d  %s' % (i, rt + extra, ds, n, pr, vs, ps, chain))
        g = groups.setdefault((p['rt'].get(0, 0), rt + extra, ds), [0, 0, 0])
        g[0] += 1
        g[1] += n
        g[2] += pr
    print('\nby target (target | depth | passes | draws | share | primitives):')
    out['targets'] = []
    for (ptr, rt, ds), (np_, n, pr) in sorted(groups.items(), key=lambda kv: -kv[1][1]):
        out['targets'].append({'target': rt, 'ptr': ptr, 'depth': ds, 'passes': np_, 'draws': n, 'primitives': pr})
        if n:
            print('  %08x %-22s %-16s %3d %5d %5.1f %% %8d' % (ptr, rt, ds, np_, n, 100.0 * n / max(total, 1), pr))

    # By the code that issued the draw.
    print('\nby the game code that issued the draw (nearest return addresses | draws | share | primitives):')
    by_caller = collections.Counter()
    by_caller_prims = collections.Counter()
    depth = opts.get('caller_depth', 3)
    for d in f.draws:
        by_caller[d['callers'][:depth]] += 1
        by_caller_prims[d['callers'][:depth]] += d['prims']
    out['callers'] = []
    for chain, n in by_caller.most_common(opts.get('top', 25)):
        text = ' '.join('%06x' % a for a in chain)
        out['callers'].append({'chain': text, 'draws': n, 'primitives': by_caller_prims[chain]})
        print('  %-30s %5d %5.1f %% %8d' % (text, n, 100.0 * n / max(total, 1), by_caller_prims[chain]))

    # By shader pair.
    print('\nby shaders (vs | ps | draws | share | primitives | constants of the pixel shader):')
    by_sh = collections.Counter()
    by_sh_prims = collections.Counter()
    for d in f.draws:
        by_sh[(d['vs'], d['ps'])] += 1
        by_sh_prims[(d['vs'], d['ps'])] += d['prims']
    out['shaders'] = []
    for (vs, ps), n in by_sh.most_common(opts.get('top', 25)):
        names = [c for c in f.shader_constants(ps)][:6]
        out['shaders'].append({'vs': f.shader_name(vs), 'ps': f.shader_name(ps), 'draws': n,
                               'primitives': by_sh_prims[(vs, ps)], 'ps_constants': f.shader_constants(ps),
                               'vs_constants': f.shader_constants(vs)})
        print('  %-16s %-16s %5d %5.1f %% %8d  %s' % (f.shader_name(vs), f.shader_name(ps), n,
                                                     100.0 * n / max(total, 1), by_sh_prims[(vs, ps)],
                                                     ' '.join(names)))
    print('  distinct pairs: %d, vertex shaders %d, pixel shaders %d' % (
        len(by_sh), len({d['vs'] for d in f.draws}), len({d['ps'] for d in f.draws})))

    # What changes between consecutive draws.
    print('\nwhat changed since the previous draw (kinds | draws | share):')
    ch = collections.Counter()
    for d in f.draws:
        kinds_ = tuple(sorted(k for k in d['changed'] if not k[-1].isdigit()))
        ch[kinds_] += 1
    out['changes'] = []
    for kinds_, n in ch.most_common(15):
        out['changes'].append({'kinds': list(kinds_), 'draws': n})
        print('  %-40s %5d %5.1f %%' % (' '.join(kinds_) or '(nothing)', n, 100.0 * n / max(total, 1)))
    per_draw = collections.Counter()
    for d in f.draws:
        for op, n in d['calls'].items():
            per_draw[op] += n
    print('  calls a draw on average: %.1f' % (ncalls / max(total, 1)))

    # Merging and instancing.
    merge_same_state = 0       # nothing changed at all since the previous draw
    merge_contiguous = 0       # ... and the indices follow each other
    inst_consecutive = 0       # same mesh, same material, only vertex constants changed
    for prev, d in zip(f.draws, f.draws[1:]):
        if prev['pass'] != d['pass'] or d['op'] != 82 or prev['op'] != 82:
            continue
        kinds_ = {k for k in d['changed'] if not k[-1].isdigit()}
        if not kinds_:
            merge_same_state += 1
            a, b = prev['args'], d['args']
            if a[0] == b[0] == 4 and a[1] == b[1] and a[4] + 3 * a[5] == b[4]:
                merge_contiguous += 1
        if kinds_ <= {'vsc'} and mesh_key(prev) == mesh_key(d):
            inst_consecutive += 1
    # Runs of draws that differ in nothing but the range of indices: what
    # the module's merging (`draw_merge`) joins into one draw.
    runs = []
    run = []
    for d in f.draws:
        kinds_ = {k for k in d['changed'] if not k[-1].isdigit()}
        joins = (run and d['op'] == 82 and not kinds_ and d['pass'] == run[-1]['pass']
                 and d['args'][0] == run[-1]['args'][0] and d['args'][0] in (4, 5)
                 and d['args'][1] == run[-1]['args'][1])
        if joins:
            run.append(d)
        else:
            if len(run) > 1:
                runs.append(run)
            run = [d] if d['op'] == 82 else []
    if len(run) > 1:
        runs.append(run)
    out['runs'] = {}
    print('\nruns of indexed draws with nothing changed between them (at least N draws | runs | draws saved | share '
          '| indices to copy):')
    for least in (2, 3, 4, 6, 8):
        sel = [r for r in runs if len(r) >= least]
        saved = sum(len(r) - 1 for r in sel)
        indices = sum(sum(d['prims'] + 2 for d in r) for r in sel)
        out['runs'][least] = {'runs': len(sel), 'saved': saved, 'indices': indices}
        print('  %2d %5d %5d %5.1f %% %8d' % (least, len(sel), saved, 100.0 * saved / max(total, 1), indices))
    by_pass = collections.Counter()
    for r in runs:
        by_pass[r[0]['pass']] += len(r) - 1
    print('  by pass: ' + ', '.join('%d: %d' % kv for kv in by_pass.most_common(8)))
    print('  by primitive type: ' + ', '.join('%s: %d' % (
        {4: 'list', 5: 'strip'}[k], n) for k, n in collections.Counter(
            r[0]['args'][0] for r in runs for _ in r[1:]).items()))
    dyn = sum(len(r) - 1 for r in runs
              if any(f.buffers.get(b, {}).get('usage', 0) & 0x200
                     for b in [r[0]['ib']] + [s[1][0] for s in r[0]['streams']]))
    print('  of them on dynamic buffers: %d' % dyn)
    # Instancing regardless of order: same mesh and material within a pass.
    groups_i = collections.Counter()
    for d in f.draws:
        if d['op'] == 82:
            groups_i[(d['pass'], mesh_key(d), material_key(d))] += 1
    inst_any = sum(n - 1 for n in groups_i.values() if n > 1)
    inst_groups = sum(1 for n in groups_i.values() if n > 1)
    # Same mesh drawn in several passes (depth, shadow cascades, colour).
    mesh_passes = collections.defaultdict(set)
    for d in f.draws:
        if d['op'] == 82:
            mesh_passes[(mesh_key(d), d['vsc'][:0])].add(d['pass'])
    out['merge'] = {'same_state_consecutive': merge_same_state, 'contiguous_indices': merge_contiguous,
                    'instancing_consecutive': inst_consecutive, 'instancing_within_pass': inst_any,
                    'instancing_groups': inst_groups}
    print('\nmerging and instancing (indexed draws):')
    print('  nothing changed since the previous draw (could be one draw with a joined index list): %d (%.1f %%)' % (
        merge_same_state, 100.0 * merge_same_state / max(total, 1)))
    print('    of them with indices that follow each other (one call as is): %d' % merge_contiguous)
    print('  same mesh as the previous draw, only vertex constants changed (instancing in order): %d (%.1f %%)' % (
        inst_consecutive, 100.0 * inst_consecutive / max(total, 1)))
    print('  same mesh and material anywhere in the pass (instancing after sorting): %d draws saved in %d groups (%.1f %%)' % (
        inst_any, inst_groups, 100.0 * inst_any / max(total, 1)))
    distinct_meshes = len({mesh_key(d) for d in f.draws if d['op'] == 82})
    print('  distinct meshes (buffers and range): %d; a mesh is drawn %.2f times a frame' % (
        distinct_meshes, kinds.get(82, 0) / max(distinct_meshes, 1)))
    out['distinct_meshes'] = distinct_meshes
    vbs = collections.Counter(d['streams'][0][1][0] if d['streams'] else 0 for d in f.draws if d['op'] == 82)
    print('  vertex buffers used: %d; index buffers: %d' % (len(vbs), len({d['ib'] for d in f.draws if d['op'] == 82})))
    # Which vertex constants differ between instances of one mesh in a pass.
    reg_changes = collections.Counter()
    pairs = 0
    last = {}
    for d in f.draws:
        if d['op'] != 82:
            continue
        k = (d['pass'], mesh_key(d), material_key(d))
        if k in last:
            pairs += 1
            a = last[k]
            for r, x, y in zip(d['vs_used'], a['vsc'], d['vsc']):
                if x != y:
                    reg_changes[r] += 1
        last[k] = d
    if pairs:
        print('  vertex constants that differ between two draws of one mesh and material (register: pairs of %d): %s' % (
            pairs, ', '.join('c%d: %d' % kv for kv in sorted(reg_changes.items()))))
    return out


def list_draws(f, index):
    p = f.passes[index]
    for d in p['draws']:
        print('%s %-34s prims %5d  %s %s  changed: %s  callers: %s' % (
            NAMES[d['op']][:12], d['args'], d['prims'], f.shader_name(d['vs']), f.shader_name(d['ps']),
            ' '.join(sorted(k for k in d['changed'] if not k[-1].isdigit())),
            ' '.join('%06x' % a for a in d['callers'][:5])))


def main():
    args = sys.argv[1:]
    if not args:
        raise SystemExit(__doc__)
    path = args[0]
    opts = {}
    exe = None
    as_json = False
    draws_of = None
    i = 1
    while i < len(args):
        a = args[i]
        if a == '--exe':
            exe = args[i + 1]
            i += 1
        elif a == '--passes':
            opts['passes'] = True
        elif a == '--top':
            opts['top'] = int(args[i + 1])
            i += 1
        elif a == '--depth':
            opts['caller_depth'] = int(args[i + 1])
            i += 1
        elif a == '--draws':
            draws_of = int(args[i + 1])
            i += 1
        elif a == '--json':
            as_json = True
        i += 1
    f = Frame(open(path, 'rb').read(), exe)
    if draws_of is not None:
        list_draws(f, draws_of)
        return
    if as_json:
        import io
        keep = sys.stdout
        sys.stdout = io.StringIO()
        out = report(f, opts)
        sys.stdout = keep
        json.dump(out, sys.stdout, indent=1)
    else:
        report(f, opts)


if __name__ == '__main__':
    main()

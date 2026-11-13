# Per-attribute types and message direction from the decompiled per-class
# attribute readers (callers of the packet iterator FUN_0043aff0).
import sys, re, glob, json
from collections import Counter

LISTEN = r'FUN_004089b0|FUN_00407e60|FUN_004084d0|FUN_00473ec0'
SETID = r'FUN_00408980|FUN_00408260'
out = {}
stats = Counter()
files = sorted(set(sum([glob.glob(d + '/*.c') for d in sys.argv[3:]], glob.glob(sys.argv[1] + '/*.c'))))
for f in files:
    t = open(f).read()
    reader = f.split('/')[-1][:-2]
    calls = list(re.finditer(r'FUN_0043aff0\([^;]*?,(0x[0-9a-f]+|\d+)\);', t))
    for ci, m in enumerate(calls):
        h = int(m.group(1), 0)
        end = calls[ci + 1].start() if ci + 1 < len(calls) else len(t)
        body = t[m.end():end]
        parts = re.split(r'\n\s*case (?:\(\w+ \*\))?(0x[0-9a-f]+|\d+):', body)
        if len(parts) < 3:
            parts = re.split(r'if \(\w+ == (?:\(\w+ \*\))?(0x[0-9a-f]+|\d+)\)', body)
        for k in range(1, len(parts) - 1, 2):
            idx = int(parts[k], 0)
            blk = parts[k + 1]
            b = blk.split('break;')[0]
            for lab in re.findall(r'goto (LAB_[0-9a-f]+);', b):
                j = t.find(lab + ':')
                if j >= 0:
                    b += t[j:j + 600].split('break;')[0]
            field = re.findall(r'param_1 \+ (0x[0-9a-f]+|\d+)\)?', b)
            d = None
            if re.search(r'FUN_0043ac60|FUN_0043ad90', b):
                ty = 'matrix'
            elif re.search(r'FUN_0043abc0|FUN_0043ad10|DAT_0120e700', b):
                ty = 'guid128'
            elif re.search(r'FUN_0043abe0|DAT_00e2e5d0', b):
                ty = 'vec3'
            elif re.search(r'FUN_0043ab90|DAT_00e2a89b', b) or re.search(SETID + '|' + LISTEN + '|FUN_004dafd0', b):
                if not re.search(SETID + '|' + LISTEN, b) and 'FUN_004dafd0' in b:
                    ty = 'hashed'
                elif re.search(SETID + '|' + LISTEN, b):
                    ty = 'eventid'
                    if re.search(LISTEN, b) or (re.search('FUN_00407da0', b) and '0x14' in b):
                        d = 'listen'
                    else:
                        d = 'set'
                else:
                    ty = 'string'
            elif re.search(r'\(float\)|float \*|fVar', b):
                ty = 'float'
            elif re.search(r'== 0\)|!= 0\)', b) and re.search(r'\| (0x[0-9a-f]+|\d+);|= 1;|= 0;', b):
                ty = 'bool'
            else:
                ty = 'scalar'
            key = '%08x:%d' % (h, idx)
            if key not in out:
                out[key] = {'type': ty, 'dir': d, 'field': field[:3], 'reader': reader}
                stats[(ty, d)] += 1
print(stats, len(out))
json.dump(out, open(sys.argv[2], 'w'), indent=0, sort_keys=True)

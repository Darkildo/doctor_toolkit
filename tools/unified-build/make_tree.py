"""Make the installed tree of an edition: make_tree.py <map.tsv> <cab dir> <disc dir> <out dir> [--link]
Files from cabinets are copied; files stored on the disc are symlinked (--link) or copied."""
import sys, os, shutil
m, cabs, disc, out = sys.argv[1:5]; link = '--link' in sys.argv
skip = ('Support/', 'TeamSpeak/')
n = 0
for l in open(m, encoding='utf8'):
    inst, cab, src, size = l.rstrip('\n').split('\t')
    if inst.startswith(skip) or cab.startswith('#'): continue
    dst = os.path.join(out, inst); os.makedirs(os.path.dirname(dst), exist_ok=True)
    s = os.path.join(cabs, src) if cab != '-' else os.path.join(disc, src)
    if not os.path.exists(s): print('missing', s); continue
    if os.path.lexists(dst): os.remove(dst)
    if cab == '-' and link: os.symlink(s, dst)
    else: shutil.copyfile(s, dst)
    n += 1
print('files', n)

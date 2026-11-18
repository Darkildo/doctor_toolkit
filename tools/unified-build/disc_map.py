"""Build the map 'installed path -> source' of a Godfather II disc from the exported MSI tables.
usage: disc_map.py <idt dir> > map.tsv   (columns: installed path, cabinet or '-', name in cabinet / path on disc, size)"""
import sys, os, csv
d = sys.argv[1]
def rows(t):
    with open(os.path.join(d, t + '.idt'), encoding='cp1252', newline='') as f:
        ls = f.read().split('\r\n')
    return [l.split('\t') for l in ls[3:] if l]
dirs = {r[0]: (r[1], r[2]) for r in rows('Directory')}
def long(n): return n.split('|')[-1]
def path(k, src=False):
    parts = []
    while k and k in dirs:
        parent, dd = dirs[k]
        if k == 'INSTALLLOCATION' and not src: break
        t = dd.split(':')
        name = long(t[-1] if src else t[0])
        if k in ('TARGETDIR',): break
        if name != '.': parts.append(name)
        k = parent
    return '/'.join(reversed(parts))
comp = {r[0]: r[3 - 1 + 0] for r in rows('Component')}  # Component, ComponentId, Directory_
media = sorted((int(r[1]), r[3]) for r in rows('Media'))
def cab(seq):
    for last, c in media:
        if seq <= last: return c or '-'
    return '-'
for r in rows('File'):
    fid, c, name, size, seq = r[0], r[1], long(r[2]), r[3], int(r[7])
    dk = comp[c]
    inst = '/'.join(x for x in (path(dk), name) if x)
    cb = cab(seq)
    srcname = fid if cb != '-' else '/'.join(x for x in (path(dk, True), name) if x)
    print('\t'.join((inst, cb, srcname, size)))

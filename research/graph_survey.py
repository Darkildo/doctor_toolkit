#!/usr/bin/env python3
"""Checks `graph` resources dumped with `doctor-toolkit dump --type graph`.

Usage: graph_survey.py <dump_dir> [--paths]

Prints counters; every check below holds on all 2 178 graphs of GF2 PC
(docs/formats/navigation-graph.md). `--paths` also follows every entry of
the precomputed path tables (slow, about half a minute).

- header of 0x94 bytes, version 16, sections at the offsets of a cursor walk:
  neighbour GUIDs, nodes, neighbour pointers (8 bytes per slot), links,
  neighbour links, properties, sets, instances, values, 4 zero bytes,
  visibility, then from align4(end + 3) neighbour GUID indices and the path
  table, the R-tree on 16 bytes and its indices; nothing after;
- node and link runs consecutive, link source = its node, neighbour slot
  indices the identity, unused bytes zero;
- link length = distance between its nodes;
- R-tree in preorder, binary, leaves of at most 4 node indices whose box is
  the box of the node positions, threads as described;
- visibility bits all set;
- path table: following the first links reaches the target along a
  shortest path, 0 exactly for unreachable pairs.
"""
import collections
import heapq
import math
import os
import struct
import sys

H = "<8sIHH16sIIIHHIIIHHIIIHH" + "IIIIIIII" + "III" + "IIHH" + "IIHH"
NAMES = ["pre", "ver", "nn", "nc", "guid", "nodes", "conns", "guids", "nguids", "nnc", "nconns",
         "ngidx", "nptrs", "nneigh", "nrt", "ppath", "rtidx", "rtnodes", "gpi", "gflags",
         "props", "nprops", "psets", "npsets", "pinst", "npinst", "pdata", "npdata",
         "vis", "nvis", "rle", "lights", "lsets", "nlights", "nlsets", "hid", "ext", "next", "pad"]
NONE = 0xFFFF


def align(x, a):
    return (x + a - 1) // a * a


def layout(h):
    """Section offsets as the writer places them."""
    n, c = h["nn"], h["nc"]
    slots = h["nneigh"]
    at = {}
    cur = 0x94

    def place(key, size, present=True):
        nonlocal cur
        at[key] = cur if present else 0
        if present:
            cur += size

    place("guids", 16 * h["nguids"], h["nguids"] > 0)
    place("nodes", 36 * n, n > 0)
    place("nptrs", 8 * slots, slots > 0)
    place("conns", 16 * c, c > 0)
    place("nconns", 16 * h["nnc"], h["nnc"] > 0)
    has_props = h["npsets"] > 0
    place("props", 8 * h["nprops"], has_props)
    place("psets", 8 * h["npsets"], has_props)
    place("pinst", 8 * h["npinst"], has_props)
    place("pdata", h["npdata"])
    cur = align(cur, 4) + 4
    place("vis", h["nvis"])
    cur = align(cur + 3, 4)
    place("ngidx", 2 * (slots - h["next"]), slots - h["next"] > 0)
    place("ppath", (n * n + 1) // 2, h["ppath"] != 0)
    if h["nrt"]:
        cur = align(cur, 16)
    place("rtnodes", 36 * h["nrt"], h["nrt"] > 0)
    place("rtidx", 2 * n, h["nrt"] > 0)
    return at, cur


def shortest(adj, start):
    dist = {start: 0.0}
    pq = [(0.0, start)]
    while pq:
        d, u = heapq.heappop(pq)
        if d > dist[u]:
            continue
        for v, length in adj[u]:
            nd = d + length
            if nd < dist.get(v, math.inf):
                dist[v] = nd
                heapq.heappush(pq, (nd, v))
    return dist


def check(d, c, paths):
    h = dict(zip(NAMES, struct.unpack_from(H, d, 0)))
    n, cc = h["nn"], h["nc"]
    c["graphs"] += 1
    c["version 16"] += h["ver"] == 16
    at, end = layout(h)
    c["layout as written"] += all(h[k] == v for k, v in at.items()) and end == len(d)
    nodes = [struct.unpack_from("<4fHHBBHHhB7s", d, h["nodes"] + 36 * k) for k in range(n)]
    links = [struct.unpack_from("<fHHHH4s", d, h["conns"] + 16 * k) for k in range(cc)]
    run = nrun = 0
    ok = True
    for i, node in enumerate(nodes):
        ok &= node[4] == run and node[9] == 0 and node[10] == 0 and node[12] == bytes(7)
        ok &= node[8] == nrun
        ok &= all(links[e][2] == i for e in range(node[4], node[4] + node[6]))
        run += node[6]
        nrun += node[7] + node[11]
    c["node runs and sources"] += ok and run == cc and nrun == h["nnc"]
    c["link length = distance"] += all(
        abs(L - math.dist(nodes[s][:3], nodes[t][:3])) < 1e-4 for L, t, s, _, _, _ in links)
    c["neighbour indices identity"] += all(
        struct.unpack_from("<H", d, h["ngidx"] + 2 * k)[0] == k for k in range(h["nneigh"] - h["next"]))
    vis = d[h["vis"]:h["vis"] + h["nvis"]]
    c["visibility all set"] += h["rle"] == 0 and all(b == 0xFF for b in vis) and \
        h["nvis"] == math.ceil(n * (n - 1) / 2 / 8)
    if n:
        r = [struct.unpack_from("<6f6H", d, h["rtnodes"] + 36 * k) for k in range(h["nrt"])]
        idx = [struct.unpack_from("<H", d, h["rtidx"] + 2 * k)[0] for k in range(n)]

        def escape(k):
            while k != NONE:
                if r[k][8] != NONE:
                    return r[k][8]
                k = r[k][6]
            return NONE
        ok = sorted(idx) == list(range(n)) and len(r) == 2 * sum(1 for x in r if x[11]) - 1
        first = 0
        for k, x in enumerate(r):
            ok &= x[7] in (NONE, k + 1)
            ok &= x[9] == (NONE if x[8] != NONE else escape(k))
            if x[7] == NONE:
                pts = [nodes[i] for i in idx[x[10]:x[10] + x[11]]]
                ok &= x[10] == first and 0 < x[11] <= 4
                ok &= all(x[a] == min(p[a] for p in pts) and x[3 + a] == max(p[a] for p in pts)
                          for a in range(3))
                first += x[11]
        c["R-tree shape"] += ok
    if paths and h["ppath"] and n:
        pp = d[h["ppath"]:h["ppath"] + (n * n + 1) // 2]
        adj = [[(links[e][1], links[e][0]) for e in range(x[4], x[4] + x[6])] for x in nodes]
        dist = [shortest(adj, i) for i in range(n)]
        ok = True
        for i in range(n):
            for j in range(n):
                k = i * n + j
                v = (pp[k // 2] >> (4 * (k % 2))) & 15
                if i == j or j not in dist[i]:
                    ok &= v == 0
                    continue
                if v == 0 or v > len(adj[i]):
                    ok = False
                    continue
                t, length = adj[i][v - 1]
                ok &= length + dist[t].get(j, math.inf) <= dist[i][j] * (1 + 1e-5) + 1e-4
        c["paths shortest"] += ok
        c["graphs with paths"] += 1


def main(root, paths):
    c = collections.Counter()
    for dp, _, fns in os.walk(root):
        for fn in sorted(fns):
            with open(os.path.join(dp, fn), "rb") as f:
                check(f.read(), c, paths)
    for k in sorted(c):
        print(f"{k:28} {c[k]}")


if __name__ == "__main__":
    main(sys.argv[1], "--paths" in sys.argv[2:])

#!/usr/bin/env python3
"""Checks the header of `graph` resources dumped with `doctor-toolkit dump --type graph`.

Usage: graph_survey.py <dump_dir>

Observed layout (little-endian, absolute offsets):

    0x00  8 zero bytes
    0x08  u32 0x10: offset of the graph GUID
    0x0C  u16 node count, u16 edge count (of the first layer)
    0x10  16-byte GUID; SimGroup attributes (PatrolRelay.0, ...) reference it
    0x20  three layer descriptors of 16 bytes:
          u32 *nodes, u32 *edges, u32 *, u16, u16
    0x88  u32 0x3C572A4E = sdbm("graph")

Node records are 0x24 bytes: f32 x, y, z, f32 radius, u16 first edge,
u16 0xFFFF, u8 edge count, u8 flags, u16 node index, 12 bytes.
Edge records are 0x10 bytes: f32 length, u16 target node, ...
"""
import collections
import os
import struct
import sys

TYPE_GRAPH = 0x3C572A4E


def main(root):
    c = collections.Counter()
    for dp, _, fns in os.walk(root):
        for fn in fns:
            d = open(os.path.join(dp, fn), "rb").read()
            c["files"] += 1
            pre, guid_at, nodes, edges = struct.unpack_from("<QIHH", d, 0)
            c["preamble_zero"] += pre == 0
            c["guid_at_0x10"] += guid_at == 0x10
            c["type_marker_at_0x88"] += struct.unpack_from("<I", d, 0x88)[0] == TYPE_GRAPH
            c["guid_has_creator"] += all(48 <= b < 91 or b == 95 for b in d[0x18:0x20])
            p_nodes, p_edges = struct.unpack_from("<II", d, 0x20)
            if nodes:
                c["graphs_with_nodes"] += 1
                recs = []
                for k in range(nodes):
                    at = p_nodes + 0x24 * k
                    if at + 0x24 > len(d):
                        break
                    first, ffff, count = struct.unpack_from("<HHB", d, at + 0x10)
                    recs.append((first, ffff, count))
                if len(recs) != nodes:
                    c["nodes_out_of_range"] += 1
                    continue
                c["node_ffff_at_0x12"] += all(r[1] == 0xFFFF for r in recs)
                c["edge_counts_sum_to_header"] += sum(r[2] for r in recs) == edges
                run = 0
                ok = True
                for first, _, count in recs:
                    ok &= first == run
                    run += count
                c["first_edge_is_running_sum"] += ok
                c["edges_follow_nodes"] += p_edges == p_nodes + 0x24 * nodes
                targets_ok = all(
                    struct.unpack_from("<H", d, p_edges + 0x10 * e + 4)[0] < nodes for e in range(edges)
                ) if p_edges + 0x10 * edges <= len(d) else False
                c["edge_target_below_node_count"] += targets_ok
    for k in sorted(c):
        print(f"{k:28} {c[k]}")


if __name__ == "__main__":
    main(sys.argv[1])

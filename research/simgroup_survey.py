#!/usr/bin/env python3
"""Checks the SimGroup layout on resources dumped with `doctor-toolkit dump --type simgroup`.

Usage: simgroup_survey.py <dump_dir>

Layout being checked (all little-endian, offsets from the start of the resource):

    0x00  8 zero bytes
    0x08  "GmiS" (u32 0x53696D47), u32 version 1, u32 group hash, u32 flags 1
    0x18  u32 object count N, u32 0, u32 aux size, u32 string size, u32 table offset 0x30, u32 0
    0x30  N x u32 object offsets
          aux region (guids, vectors, matrices), string region (starts with NUL),
          attribute packets, objects (to the end of the resource)

Every pointer inside the resource is self-relative: target = position of the field + value.
"""
import collections
import os
import struct
import sys


def packets(d, p, end):
    out = {}
    while p < end:
        h, na = struct.unpack_from("<IH", d, p)
        mb = (na + 7) // 8
        mask = int.from_bytes(d[p + 6:p + 6 + mb], "little")
        hl = (6 + mb + 3) & ~3
        vals = [i for i in range(na) if not (mask >> i) & 1]
        out[p] = (h, na, mask, [p + hl + 4 * k for k in range(len(vals))], vals)
        p += hl + 4 * len(vals)
    return out, p


def rel(d, q):
    return q + struct.unpack_from("<i", d, q)[0]


def main(root):
    c = collections.Counter()
    gaps = collections.Counter()
    for dp, _, fns in os.walk(root):
        for fn in fns:
            d = open(os.path.join(dp, fn), "rb").read()
            pre, magic, ver, gid, flags, n, z1, auxsz, strsz, tbl, z2 = struct.unpack_from("<Q10I", d, 0)
            c["files"] += 1
            c["header_constants_ok"] += (pre, magic, ver, flags, z1, tbl, z2) == (0, 0x53696D47, 1, 1, 0, 0x30, 0)
            aux = tbl + 4 * n
            strs = aux + auxsz
            pk = strs + strsz
            offs = struct.unpack_from("<%dI" % n, d, tbl)
            first = offs[0] if n else len(d)
            c["string_region_starts_with_nul"] += strsz == 0 or d[strs] == 0
            c["string_region_ends_with_nul"] += strsz == 0 or d[pk - 1] == 0
            units, end = packets(d, pk, first)
            c["packets_end_at_first_object"] += end == first
            # Objects are contiguous up to the end of the resource.
            pos = first
            contiguous = True
            aux_targets = collections.defaultdict(set)
            str_targets = set()
            used_units = set()
            for o in offs:
                contiguous &= o == pos
                f0, f1, ng, npk = d[o + 0x1C:o + 0x20]
                c["object_prefix_zero"] += d[o:o + 0x1C] == bytes(0x1C)
                c["object_flags:%d/%d" % (f0, f1)] += 1
                comp = struct.unpack_from("<i", d, o + 0x20)[0]
                c["components_iff_flag8"] += (comp != 0) == bool(f0 & 8)
                if comp:
                    t = o + 0x20 + comp
                    c["components_point_at_packet"] += t in units
                    used_units.add(t)
                g = rel(d, o + 0x24)
                c["guid_in_aux"] += aux <= g < strs
                aux_targets[g].add("guid")
                for k in range(ng):
                    t = rel(d, o + 0x2C + 4 * k)
                    c["link_in_aux"] += aux <= t < strs
                    aux_targets[t].add("link")
                for k in range(ng, ng + npk):
                    t = rel(d, o + 0x2C + 4 * k)
                    c["packet_ref_ok"] += t in units
                    used_units.add(t)
                    if t in units and k == ng + npk - 1:
                        c["last_packet_is_object_class"] += units[t][0] == struct.unpack_from("<I", d, o + 0x28)[0]
                pos = o + 0x2C + 4 * (ng + npk)
            c["objects_contiguous_to_end"] += contiguous and pos == len(d)
            c["all_packets_used"] += set(units) <= used_units
            for p, (h, na, mask, slots, vals) in units.items():
                for q in slots:
                    v = struct.unpack_from("<i", d, q)[0]
                    t = q + v
                    if v and aux <= t < strs:
                        aux_targets[t].add("value")
                        c["value_kind:aux"] += 1
                    elif v and strs <= t < pk:
                        str_targets.add(t)
                        c["value_kind:string"] += 1
                        c["string_target_after_nul"] += d[t - 1] == 0
                    else:
                        c["value_kind:scalar"] += 1
            # Aux blobs: distance to the next referenced blob.
            ts = sorted(aux_targets) + [strs]
            c["aux_starts_at_region_start"] += not aux_targets or ts[0] == aux
            for a, b in zip(ts, ts[1:]):
                for kind in aux_targets[a]:
                    gaps[(kind, b - a)] += 1
            # Every string of the region is referenced.
            pos = strs + 1
            all_ref = True
            while pos < pk:
                e = d.index(b"\0", pos)
                all_ref &= pos in str_targets
                pos = e + 1
            c["every_string_referenced"] += all_ref
    for k in sorted(c):
        print(f"{k:36} {c[k]}")
    print("aux blob sizes (kind, distance to next blob): count")
    for k in sorted(gaps, key=lambda k: (k[0], k[1])):
        print(f"  {k}: {gaps[k]}")


if __name__ == "__main__":
    main(sys.argv[1])

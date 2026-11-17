#!/usr/bin/env python3
"""The people the player can hire: the `NPC_CRW_*` templates of the game files
and everything that points at them (stage 82, docs/kb/systems/crew.md).

Usage:
    crew_survey.py <simgroup_dump_dir>            the templates
    crew_survey.py <simgroup_dump_dir> refs       who points at the templates
    crew_survey.py <simgroup_dump_dir> names      NPC name prefixes of the game

The dump is made with `doctor-toolkit dump --type simgroup -o <dir> GAME`.

A template is a `CommonNPC` object whose attribute `NPC.2` (the name of the
text of the character's name) starts with `NPC_CRW_`. Printed per template:
name, GUID, the appearance preset (`PartedAnimated.0` and its group
`PartedAnimated.1`), the word of `NPCCrewComponent.0` with the specialties its
bits 0x04..0x80 give (the reader 0x00790000 keeps `& 0xFC`), the health, the
message the hire sends for a single specialty (`NPC.31`), the hash of
`NPC.18` (the text of the name: `sdbm("name_" + NPC.2)`), the rank
(`Sentient.1`) and the flags of the template's `SimNpc` (`SimNpc.0`: bit 3
and bit 4 give the weapon licence the man starts with).

`refs` prints every attribute of every object of the game that holds the GUID
of a template or of the `SimNpc` of a template (the recruits in the world are
`SpecificNpcRequester` objects that ask for the SimNpc): stream, class of the
object, packet and attribute, template; and the counts by class.
"""
import collections
import os
import struct
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
from class_usage import groups, names, packet, rel, sdbm  # noqa: E402

SPECIALTY = [(0x04, "demolition"), (0x08, "arsonist"), (0x10, "safe_cracker"),
             (0x20, "engineer"), (0x40, "medic"), (0x80, "bruiser")]


def specialties(mask):
    return "+".join(n for b, n in SPECIALTY if mask & b) or "-"


def value(d, q, areas):
    """An attribute value: ('str', text), ('guid', hex) or ('raw', u32)."""
    aux, strs, end = areas
    raw = struct.unpack_from("<I", d, q)[0]
    t = q + struct.unpack_from("<i", d, q)[0]
    if raw and strs <= t < end:
        return "str", d[t:d.index(b"\0", t)].decode("latin-1")
    if raw and aux <= t < strs:
        return "guid", d[t:t + 16].hex()
    return "raw", raw


def all_objects(root):
    """(stream, class hash, guid hex, {packet hash: {index: value}})"""
    for path, d, offs, areas in groups(root):
        stream = os.path.dirname(os.path.relpath(path, root))
        for o in offs:
            ng, npk = d[o + 0x1E], d[o + 0x1F]
            cls = struct.unpack_from("<I", d, o + 0x28)[0]
            g = rel(d, o + 0x24)
            guid = d[g:g + 16].hex()
            pk = {}
            for k in range(ng, ng + npk):
                h, slots = packet(d, rel(d, o + 0x2C + 4 * k))
                pk[h] = {i: value(d, q, areas) for i, q in slots.items()}
            yield stream, cls, guid, pk


def templates(root, objects=None):
    npc, crew, parted, damage = sdbm("NPC"), sdbm("NPCCrewComponent"), sdbm("PartedAnimated"), sdbm("StandardDamageComponent")
    sentient, sim_npc = sdbm("Sentient"), sdbm("SimNpc")
    objects = list(all_objects(root)) if objects is None else objects
    # template GUID -> (SimNpc GUID, SimNpc.0)
    sims = {}
    for stream, cls, guid, pk in objects:
        if cls == sim_npc:
            a = pk.get(sim_npc, {})
            if a.get(1, ("", ""))[0] == "guid":
                sims[a[1][1]] = (guid, a.get(0, ("raw", 0))[1])
    out = []
    for stream, cls, guid, pk in objects:
        name = pk.get(npc, {}).get(2, ("", ""))
        if name[0] != "str" or not name[1].startswith("NPC_CRW_"):
            continue
        mask = pk.get(crew, {}).get(0, ("raw", 0))[1]
        hp = pk.get(damage, {}).get(0, ("raw", 0))[1]
        out.append({
            "name": name[1], "guid": guid, "stream": stream,
            "preset": pk.get(parted, {}).get(0, ("", "-"))[1],
            "group": pk.get(parted, {}).get(1, ("", "-"))[1],
            "mask": mask,
            "health": struct.unpack("<f", struct.pack("<I", hp))[0],
            "hired": pk.get(npc, {}).get(31, ("", "-"))[1],
            "hash18": pk.get(npc, {}).get(18, ("raw", 0))[1],
            "rank": pk.get(sentient, {}).get(1, ("raw", 0))[1],
            "sim": sims.get(guid, ("-", 0))[0],
            "sim_flags": sims.get(guid, ("-", 0))[1],
        })
    return sorted(out, key=lambda t: t["name"])


def main():
    root = sys.argv[1]
    mode = sys.argv[2] if len(sys.argv) > 2 else "templates"
    if mode == "names":
        npc = sdbm("NPC")
        c = collections.Counter()
        for _, _, _, pk in all_objects(root):
            v = pk.get(npc, {}).get(2)
            if v and v[0] == "str":
                c[v[1][:7]] += 1
        for k, n in sorted(c.items()):
            print("%s\t%d" % (k, n))
        return
    objects = list(all_objects(root))
    ts = templates(root, objects)
    if mode == "templates":
        print("# name\tguid\tstream\tpreset\tgroup\tword\tspecialties\thealth\thire message\tNPC.18\trank\tSimNpc.0")
        wrong = 0
        for t in ts:
            print("%s\t%s\t%s\t%s\t%s\t0x%X\t%s\t%g\t%s\t%08x\t%d\t%d" % (
                t["name"], t["guid"], t["stream"], t["preset"], t["group"], t["mask"],
                specialties(t["mask"]), t["health"], t["hired"], t["hash18"], t["rank"], t["sim_flags"]))
            wrong += t["hash18"] != sdbm("name_" + t["name"])
        c = collections.Counter(specialties(t["mask"]) for t in ts)
        print("# templates: %d; by specialty: %s" % (len(ts), ", ".join("%s %d" % kv for kv in sorted(c.items()))),
              file=sys.stderr)
        print("# NPC.18 is not sdbm(\"name_\" + NPC.2): %d template(s)" % wrong, file=sys.stderr)
        return
    by_guid = {t["guid"]: t["name"] for t in ts}
    by_guid.update({t["sim"]: t["name"] for t in ts})
    nm = names()
    count = collections.Counter()
    print("# stream\tobject class\tobject guid\tpacket.attribute\ttemplate")
    for stream, cls, guid, pk in objects:
        for h, attrs in pk.items():
            for i, v in attrs.items():
                if v[0] == "guid" and v[1] in by_guid:
                    where = "%s.%d" % (nm.get(h, "%08x" % h), i)
                    count[(nm.get(cls, "%08x" % cls), where)] += 1
                    print("%s\t%s\t%s\t%s\t%s" % (stream, nm.get(cls, "%08x" % cls), guid, where, by_guid[v[1]]))
    for (cls, where), n in sorted(count.items()):
        print("# %s %s: %d" % (cls, where, n), file=sys.stderr)


if __name__ == "__main__":
    main()

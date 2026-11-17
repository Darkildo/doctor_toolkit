#!/usr/bin/env python3
"""Builds data/gf2/attr_types.toml: what the values of SimGroup attributes
mean, where the game files or the code tell.

Usage: attr_types.py <simgroup_dump_dir> <resource_keys.txt> > data/gf2/attr_types.toml

Inputs:

- the SimGroups of the game, dumped with `doctor-toolkit dump --type simgroup`;
- the keys of every resource of the game, one per line (`stream:type:ids`):
  `doctor-toolkit search GAME "type:<all tags of data/gf2/type_tags.txt>" --keys --limit 0`;
- data/gf2/simgroup_schema.txt (value kinds), logic_schema.toml (pins, names,
  delays), enums.toml (value lists), sim_classes.txt (class names).

Types written (messages are not: `in` and `out` pins of logic_schema.toml are
`MsgIn` and `MsgOut` already):

- `ref:<Class>`: a GUID attribute whose values are GUIDs of SimGroup objects
  that all have the packet of <Class> (the most derived class shared by at
  least REF_SHARE of the objects found; at least RESOLVED_SHARE of the values
  must be GUIDs of objects of the game);
- `res:<type>`: a GUID attribute holding a resource id repeated four times,
  where at least REF_SHARE of the ids are resources of one type;
- `enum:<list>`: a hash attribute whose every value is `sdbm(lower(name))` of
  a name of the list (family, city, emotion); a text attribute whose every
  value is a name of the list (sound_category, weapon); an integer attribute
  named `CMD_SetVenueID`, or whose values are all venue ids (at least
  VENUE_DISTINCT different ones, the largest at least VENUE_LEAST_MAX);
- `seconds`: the delay attribute of an `out` pin (the bus takes the delay in
  seconds: 0x00440590 queues `now + delay × 1000 ms`);
- `bool`: a named integer attribute that only ever holds 0 or 1 in the game
  files — a guess;
- MANUAL below: types read from documents, with their source;
- data/gf2/class_behavior.toml (attributes as the code of the classes uses
  them): kind `bool` gives the type `bool`, `unit = "seconds"` the type
  `seconds`, both with confidence `exe`.

Confidence: `exe` — the code, `data` — the game files (at least MIN_VALUES
values seen), `guess` — fewer values or an assumption. `seen` is
"values that fit / values in the game files".
"""
import collections
import os
import struct
import sys
import tomllib

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from simgroup_survey import packets, rel  # noqa: E402

HERE = os.path.dirname(os.path.abspath(__file__))
DATA = os.path.join(HERE, '..', 'data', 'gf2')
REF_SHARE = 0.95
RESOLVED_SHARE = 0.9
MIN_VALUES = 3
VENUE_DISTINCT = 5
VENUE_LEAST_MAX = 20
# Packets every object has: a reference "to an object with the Base packet"
# says nothing.
TRIVIAL = {'CAttributeHandler', 'Base'}
# Words in an attribute name that rule out a yes/no value.
NOT_BOOL = ('flag', 'type', 'mask', 'option', 'count', 'max', 'number', 'id', 'index', 'style',
            'comparison', 'operation', 'checkpoint', 'chapter', 'value', 'operand', 'score')

# (class, attribute): (type, confidence, source). Hand-read; keep it short.
MANUAL = {
    ('MissionTimer', 3): ('seconds', 'exe', 'docs/logic-graph.md §2: reader of MissionTimer, attribute named `seconds`'),
    ('FadeScreenFx', 2): ('seconds', 'guess', 'docs/logic-graph.md §2: name `seconds` is a guess'),
    ('Family', 18): ('seconds', 'guess', 'docs/analysis/tunables.md §3: turn timers of a family, which one is not known'),
    ('Family', 19): ('seconds', 'guess', 'docs/analysis/tunables.md §3: turn timers of a family, which one is not known'),
    ('Family', 20): ('seconds', 'guess', 'docs/analysis/tunables.md §3: turn timers of a family, which one is not known'),
    ('Family', 21): ('seconds', 'guess', 'docs/analysis/tunables.md §3: turn timers of a family, which one is not known'),
    ('Gun', 15): ('seconds', 'guess', 'docs/analysis/tunables.md §2: time between shots, not checked in the game'),
    ('Bank', 0): ('money', 'guess', 'docs/analysis/tunables.md §2: 10000 at four bank pick-ups, not checked in the game'),
    ('Bank', 1): ('money', 'guess', 'docs/analysis/tunables.md §2: 5000 at four bank pick-ups, not checked in the game'),
    ('NPC', 25): ('money', 'guess', 'docs/analysis/tunables.md §3: looks like the least money dropped, not checked'),
    ('NPC', 26): ('money', 'guess', 'docs/analysis/tunables.md §3: looks like the most money dropped, not checked'),
    ('MoneyAdaptor', 0): ('money', 'exe', 'docs/analysis/handing-out.md §4.1: the sum 0x008A6EE0 gives to AddMoney; may be negative'),
    ('AddPlayerFamilyMember', 2): ('enum:crew_rank', 'exe', 'docs/analysis/crew.md §4: the reader 0x0090A1D0 keeps 3..5'),
}


# GUID attributes whose targets in the game files all have one class, while
# the code takes any object (data/gf2/class_behavior.toml says which): the
# guessed `ref:<Class>` is dropped.
NOT_A_CLASS_REF = {
    ('MessageRelay', 3),  # 0x004CF950: any object with a message listener
    ('PlaySound', 5),  # any object with the Entity component, not the player only
}


def sdbm_lower(s):
    h = 0
    for c in s.lower().encode('latin1'):
        h = (c + h * 65599) & 0xFFFFFFFF
    return h


def load_kinds():
    kinds, names = {}, {}
    for line in open(os.path.join(DATA, 'simgroup_schema.txt')):
        line = line.strip()
        if not line or line.startswith('#'):
            continue
        c = line.split()
        kinds[(int(c[0], 16), int(c[1]))] = c[2]
        if len(c) > 4:
            names[(int(c[0], 16), int(c[1]))] = c[4]
    return kinds, names


def load_class_names():
    names = {}
    for line in open(os.path.join(DATA, 'sim_classes.txt')):
        line = line.strip()
        if line and not line.startswith('#'):
            names[sdbm_lower(line)] = line
    return names


def load_corpus(root, kinds):
    """Yields (object guid, [(packet class, {index: value})])."""
    for dp, _, fns in sorted(os.walk(root)):
        for fn in sorted(fns):
            d = open(os.path.join(dp, fn), 'rb').read()
            n, auxsz, strsz, tbl = (struct.unpack_from('<I', d, o)[0] for o in (0x18, 0x20, 0x24, 0x28))
            pk = tbl + 4 * n + auxsz + strsz
            offs = struct.unpack_from('<%dI' % n, d, tbl)
            units, _ = packets(d, pk, offs[0] if n else len(d))
            for o in offs:
                ng, npk = d[o + 0x1E], d[o + 0x1F]
                g = rel(d, o + 0x24)
                pks = []
                for k in range(ng, ng + npk):
                    h, _, _, slots, idx = units[rel(d, o + 0x2C + 4 * k)]
                    attrs = {}
                    for q, i in zip(slots, idx):
                        kind = kinds.get((h, i))
                        if kind == 'string':
                            p = rel(d, q)
                            attrs[i] = d[p:d.index(b'\0', p)].decode('latin1')
                        elif kind == 'guid':
                            p = rel(d, q)
                            attrs[i] = d[p:p + 16]
                        elif kind in ('int', 'hash', 'float'):
                            attrs[i] = struct.unpack_from('<I', d, q)[0]
                    pks.append((h, attrs))
                yield d[g:g + 16], pks


def main(root, keys_path):
    kinds, cmd_names = load_kinds()
    class_names = load_class_names()
    schema = tomllib.load(open(os.path.join(DATA, 'logic_schema.toml'), 'rb'))
    enums = tomllib.load(open(os.path.join(DATA, 'enums.toml'), 'rb'))
    packet_name = {int(p['hash'], 16): n for n, p in schema['packet'].items()}
    pins = {(int(p['hash'], 16), a['i']): a for p in schema['packet'].values() for a in p['attrs']}

    def cname(h):
        return packet_name.get(h) or class_names.get(h) or '%08x' % h

    hash_sets = {k: {sdbm_lower(v['name']) for v in enums[k]['values']} for k in ('family', 'city', 'emotion')}
    name_sets = {k: {v['name'] for v in enums[k]['values']} for k in ('sound_category', 'weapon')}
    venues = {v['value'] for v in enums['venue']['values']}

    resource_types = collections.defaultdict(set)
    for line in open(keys_path):
        _, rtype, ids = line.strip().rsplit(':', 2)
        resource_types[int(ids.split('.')[-1], 16)].add(rtype)

    chains = {}
    values = collections.defaultdict(list)
    for guid, pks in load_corpus(root, kinds):
        chains[guid] = [h for h, _ in pks]
        for h, attrs in pks:
            for i, v in attrs.items():
                values[(h, i)].append(v)

    found = {}

    def put(h, i, kind, conf, fit, total, source=None):
        found[(h, i)] = (kind, conf, fit, total, source)

    def conf_of(n):
        return 'data' if n >= MIN_VALUES else 'guess'

    for (h, i), vs in values.items():
        kind = kinds.get((h, i))
        total = len(vs)
        if kind == 'guid':
            targets = [chains[v] for v in vs if v in chains]
            ids = [struct.unpack('<I', v[:4])[0] for v in vs
                   if v not in chains and len({v[k:k + 4] for k in range(0, 16, 4)}) == 1]
            if targets and len(targets) >= RESOLVED_SHARE * total:
                # The most derived class of the first target that enough targets share.
                best = None
                for cls in targets[0]:
                    share = sum(1 for t in targets if cls in t)
                    if share >= REF_SHARE * len(targets) and cname(cls) not in TRIVIAL:
                        best = (cls, share)
                if best:
                    put(h, i, 'ref:' + cname(best[0]), conf_of(total), best[1], total)
            elif ids and len(ids) >= RESOLVED_SHARE * total:
                by_type = collections.Counter()
                for rid in ids:
                    for t in resource_types.get(rid, ()):
                        by_type[t] += 1
                if by_type:
                    rtype, n = by_type.most_common(1)[0]
                    if n >= REF_SHARE * total:
                        put(h, i, 'res:' + rtype, conf_of(total), n, total)
        elif kind == 'hash':
            for name, known in hash_sets.items():
                if all(v in known for v in vs):
                    put(h, i, 'enum:' + name, conf_of(total), total, total)
        elif kind == 'string':
            if (h, i) in pins and pins[(h, i)]['pin'] in ('in', 'out'):
                continue
            for name, known in name_sets.items():
                if all(v in known for v in vs):
                    put(h, i, 'enum:' + name, conf_of(total), total, total)
        elif kind == 'int':
            name = cmd_names.get((h, i)) or pins.get((h, i), {}).get('name')
            # Small numbers alone (1–7 are venue ids too) do not tell.
            if name == 'CMD_SetVenueID' or (all(v in venues for v in vs) and len(set(vs)) >= VENUE_DISTINCT
                                            and max(vs) >= VENUE_LEAST_MAX):
                fit = sum(1 for v in vs if v in venues)
                put(h, i, 'enum:venue', 'data' if name == 'CMD_SetVenueID' else conf_of(total), fit, total)
            elif name and all(v in (0, 1) for v in vs) and not any(w in name.lower() for w in NOT_BOOL):
                put(h, i, 'bool', 'guess', total, total)

    # Delays of `out` pins.
    for (h, i), a in pins.items():
        if 'delay' in a:
            d = (h, a['delay'])
            put(h, a['delay'], 'seconds', a.get('conf', 'data'), len(values.get(d, ())), len(values.get(d, ())),
                'delay of out pin %d (docs/logic-graph.md §2)' % i)
    for (cls, i), (kind, conf, source) in MANUAL.items():
        h = sdbm_lower(cls)
        n = len(values.get((h, i), ()))
        put(h, i, kind, conf, n, n, source)

    # What the code of the classes shows (data/gf2/class_behavior.toml): a
    # value the reader only tests for zero is a `bool`, a float marked as a
    # time is `seconds`. A `bool` guessed from the game files that the code
    # uses in another way is dropped.
    behavior = tomllib.load(open(os.path.join(DATA, 'class_behavior.toml'), 'rb'))['class']
    for cls in behavior.values():
        for a in cls.get('attr', []):
            h = int(a['packet'], 16) if len(a['packet']) == 8 and a['packet'].islower() else sdbm_lower(a['packet'])
            vs = values.get((h, a['i']), ())
            if (a['packet'], a['i']) in NOT_A_CLASS_REF and found.get((h, a['i']), ('',))[0].startswith('ref:'):
                del found[(h, a['i'])]
            if not vs:
                # Not stored in the game files: nothing to count, the file
                # of the classes says the rest.
                continue
            source = 'data/gf2/class_behavior.toml: reader %s' % a['addr']
            if a['kind'] == 'bool':
                put(h, a['i'], 'bool', 'exe', sum(1 for v in vs if v in (0, 1)), len(vs), source)
            elif a.get('unit') == 'seconds':
                put(h, a['i'], 'seconds', 'exe', len(vs), len(vs), source)
            elif found.get((h, a['i']), ('',))[0] == 'bool':
                del found[(h, a['i'])]

    out = sys.stdout
    w = out.write
    w('# Meaning of SimGroup attribute values of The Godfather II (PC 1.0.789.0).\n')
    w('# Generated by research/attr_types.py from the game files, data/gf2/enums.toml and\n')
    w('# data/gf2/logic_schema.toml; hand-read entries are in MANUAL of that script,\n')
    w('# types read in the code of the classes come from data/gf2/class_behavior.toml.\n')
    w('# Do not edit by hand.\n#\n')
    w('# [packet.<class>] hash, attrs: i (attribute index), type, conf, seen, source.\n')
    w('# type: ref:<Class> (GUID of an object with that packet), res:<type> (resource\n')
    w('# id), enum:<list of enums.toml>, seconds, money, bool. Message pins are in\n')
    w('# logic_schema.toml (in = MsgIn, out = MsgOut).\n')
    w('# conf: exe, data, guess. seen: values that fit / values in the game files.\n')
    by_class = collections.defaultdict(list)
    for (h, i), entry in found.items():
        by_class[h].append((i, entry))
    stats = collections.Counter()
    for h in sorted(by_class, key=lambda h: cname(h).lower()):
        w('\n[packet.%s]\nhash = "%08x"\nattrs = [\n' % (cname(h), h))
        for i, (kind, conf, fit, total, source) in sorted(by_class[h]):
            fields = ['i = %d' % i, 'type = "%s"' % kind, 'conf = "%s"' % conf, 'seen = "%d/%d"' % (fit, total)]
            if source:
                fields.append('source = "%s"' % source.replace('"', '\\"'))
            w('  { ' + ', '.join(fields) + ' },\n')
            stats[kind.split(':')[0]] += 1
            stats['conf ' + conf] += 1
        w(']\n')
    stats['packets'] = len(by_class)
    stats['attributes'] = len(found)
    for k, v in sorted(stats.items()):
        sys.stderr.write('%-16s %d\n' % (k, v))


if __name__ == '__main__':
    if len(sys.argv) != 3:
        sys.exit(__doc__)
    main(sys.argv[1], sys.argv[2])

#!/usr/bin/env python3
"""Builds data/gf2/logic_schema.toml: the pins of every SimGroup class.

Usage: logic_schema.py <simgroup_dump_dir> <readers.json> > data/gf2/logic_schema.toml

Inputs:

- the SimGroups of the game, dumped with `doctor-toolkit dump --type simgroup`;
- readers.json, written by research/logic_readers.py from the decompiled
  per-class attribute readers of godfather2.exe (research/ghidra/README.md).

For every attribute of every class packet that the game stores, the script
decides the pin it becomes in the logic graph:

- `in`: a message the object listens to. The reader hashes the string with
  sdbm(lower) (0x4DAFD0), interns it in the global message table (0x408260)
  and subscribes the object to it (0x4089B0, 0x407E60, 0x4084D0 or the typed
  0x473EC0) — confidence `exe`;
- `out`: a message the object sends: interned without a subscription — `exe`;
- for string attributes without such code in the reader (the string is kept
  and hashed later, or the reader was not found): when at least
  MESSAGE_SHARE of the values are names of messages that `exe` pins send or
  hear, the attribute is a message too, its direction taken from which side
  its values match — confidence `data`;
- `link`: a GUID attribute whose values are GUIDs of SimGroup objects — `data`;
- `asset`: a GUID attribute that references resources (an id repeated 4 times);
- `param`: everything else.

Names and the direction of a few attributes come from reading the decompiled
readers by hand (MANUAL below); their confidence is given there.
"""
import collections
import json
import os
import struct
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from simgroup_survey import packets, rel  # noqa: E402

HERE = os.path.dirname(os.path.abspath(__file__))
MESSAGE_SHARE = 0.5


def sdbm_lower(s):
    h = 0
    for c in s.lower().encode():
        h = (c + h * 65599) & 0xFFFFFFFF
    return h


def load_kinds():
    kinds = {}
    for line in open(os.path.join(HERE, '..', 'data', 'gf2', 'simgroup_schema.txt')):
        line = line.strip()
        if not line or line.startswith('#'):
            continue
        c = line.split()
        kinds[(int(c[0], 16), int(c[1]))] = c[2]
    return kinds


def load_names():
    names = {}
    for line in open(os.path.join(HERE, '..', 'data', 'gf2', 'sim_classes.txt')):
        line = line.strip()
        if line and not line.startswith('#'):
            names[sdbm_lower(line)] = line
    return names


def load_corpus(root, kinds):
    """Yields (object class, object guid, [(packet class, {index: value})])."""
    for dp, _, fns in os.walk(root):
        for fn in sorted(fns):
            d = open(os.path.join(dp, fn), 'rb').read()
            n, auxsz, strsz, tbl = (struct.unpack_from('<I', d, o)[0] for o in (0x18, 0x20, 0x24, 0x28))
            pk = tbl + 4 * n + auxsz + strsz
            offs = struct.unpack_from('<%dI' % n, d, tbl)
            units, _ = packets(d, pk, offs[0] if n else len(d))
            for o in offs:
                ng, npk = d[o + 0x1E], d[o + 0x1F]
                g = rel(d, o + 0x24)
                cls = struct.unpack_from('<I', d, o + 0x28)[0]
                pks = []
                for k in range(ng, ng + npk):
                    h, na, mask, slots, idx = units[rel(d, o + 0x2C + 4 * k)]
                    attrs = {}
                    for q, i in zip(slots, idx):
                        kind = kinds.get((h, i))
                        if kind == 'string':
                            p = rel(d, q)
                            attrs[i] = d[p:d.index(b'\0', p)].decode('latin1')
                        elif kind == 'guid':
                            p = rel(d, q)
                            attrs[i] = d[p:p + 16]
                        else:
                            attrs[i] = struct.unpack_from('<I', d, q)[0]
                    pks.append((h, na, attrs))
                yield cls, d[g:g + 16], pks


# Attributes read by hand from the decompiled readers (addresses are the
# readers in godfather2.exe). Value: (name, pin or None to keep the derived
# one, confidence of the pin, confidence of the name). A name of an `out` pin
# followed by `+delay=<index>` gives the attribute holding its delay.
E, D, G = 'exe', 'data', 'guess'
MANUAL = {
    # MultiManager 0x47A320: attr 0 is interned and subscribed inline
    # (0x407DA0 + use count), pairs (message, delay) 3/4 … 17/18 are copied
    # into a send list of up to 8 entries.
    'MultiManager': {0: ('trigger', 'in', E, E), 1: ('max_fires', None, E, G), 2: ('flags', None, E, G),
                     **{i: ('send_%d+delay=%d' % ((i - 1) // 2, i + 1), 'out', E, E) for i in range(3, 19, 2)},
                     **{i: ('delay_%d' % ((i - 2) // 2), 'param', E, E) for i in range(4, 19, 2)}},
    # TriggerAuto 0x478040: message 0 sent after delay 1 when the group loads.
    'TriggerAuto': {0: ('send+delay=1', 'out', E, D), 1: ('delay', 'param', E, D), 2: ('flags', None, E, G)},
    # TriggerRandom 0x47B040: attr 0 subscribed with typed data; then up to 8
    # entries (weight, message, delay); weights are summed at +0xD8.
    'TriggerRandom': {0: ('trigger', 'in', E, E),
                      **{1 + 3 * k: ('weight_%d' % (k + 1), 'param', E, E) for k in range(8)},
                      **{2 + 3 * k: ('send_%d+delay=%d' % (k + 1, 3 + 3 * k), 'out', E, E) for k in range(8)},
                      **{3 + 3 * k: ('delay_%d' % (k + 1), 'param', E, G) for k in range(8)}},
    # TriggerBase 0x4C1550: three typed subscriptions, two messages sent
    # (+0x79, +0x7B), 11 is a range (squared on load).
    'TriggerBase': {0: ('fire', 'in', E, G), 1: ('enable', 'in', E, D), 2: ('disable', 'in', E, D),
                    4: ('on_enter+delay=5', 'out', E, D), 5: ('delay', 'param', E, G),
                    6: ('max_fires', 'param', E, G), 7: ('on_exit', 'out', E, D),
                    8: ('enabled', 'param', E, G), 11: ('range', 'param', E, G)},
    'EnterExitTrigger': {0: ('on_enter', 'out', E, D), 1: ('on_exit', 'out', E, D),
                         2: ('on_inside', 'out', E, G), 3: ('delay', 'param', E, G),
                         4: ('enable', 'in', E, D), 5: ('disable', 'in', E, D)},
    'TriggerDistance': {0: ('source', 'link', D, G), 1: ('target', 'link', D, G), 2: ('enable', 'in', E, D),
                        3: ('disable', 'in', E, D), 4: ('send', 'out', D, D), 5: ('distance', 'param', E, D)},
    'TriggerFollow': {2: ('enable', 'in', E, D), 3: ('disable', 'in', E, D), 4: ('on_start', 'out', D, D),
                      5: ('on_stop', 'out', D, D)},
    # AndEventGate 0x479730: four condition names kept as strings (+0x50 …
    # +0x68), output +0x7C after delay +0x78, reset subscribed at +0x84.
    'AndEventGate': {0: ('condition_1', 'in', D, D), 1: ('condition_2', 'in', D, D),
                     2: ('condition_3', 'in', D, D), 3: ('condition_4', 'in', D, D),
                     4: ('send+delay=5', 'out', E, D), 5: ('delay', 'param', E, D), 6: ('reset', 'in', E, D)},
    'MessageRelay': {0: ('trigger', 'in', D, D), 1: ('send', 'out', E, D)},
    'ForwardSender': {0: ('trigger', 'in', D, D), 1: ('send', 'out', E, D), 2: ('send_2', 'out', E, G)},
    'VariableOperator': {0: ('trigger', 'in', E, D), 1: ('trigger_2', 'in', E, G), 2: ('variable', 'param', E, D),
                         3: ('operation', 'param', E, G), 4: ('operand', 'param', E, G)},
    # VariableSwitch 0x476FC0: 29 attributes (jump table of 29 entries, 0–28).
    # 0 subscribed with typed data; 1 the variable hash (+0x58); 3 the default
    # message, interned without a subscription (+0x60), sent after the delay 4
    # (float, +0x68) when no case matches; then 8 triples (case value, message,
    # delay) from 5. Data: attr 3 is `do_nothing` in 31 objects, attr 5 is 1
    # in 82 of 87, attr 8 is 2 in 94 of 99.
    'VariableSwitch': {0: ('trigger', 'in', E, D), 1: ('variable', 'param', E, D), 2: ('variable_name', 'param', E, D),
                       3: ('default+delay=4', 'out', E, D), 4: ('default_delay', 'param', E, G),
                       **{5 + 3 * k: ('value_%d' % (k + 1), 'param', E, D) for k in range(8)},
                       **{6 + 3 * k: ('case_%d+delay=%d' % (k + 1, 7 + 3 * k), 'out', E, D) for k in range(8)},
                       **{7 + 3 * k: ('delay_%d' % (k + 1), 'param', E, G) for k in range(8)}},
    'VariableWatcher': {0: ('activate', 'in', E, D), 1: ('deactivate', 'in', E, D), 2: ('send', 'out', E, D),
                        4: ('variable', 'param', E, D), 5: ('comparison', 'param', E, G), 6: ('value', 'param', E, G)},
    'Checkpoint': {0: ('title', 'param', E, D), 1: ('number', 'param', E, D), 2: ('chapter', 'param', E, D),
                   7: ('load', 'in', E, D), 8: ('on_loaded', 'out', E, D), 9: ('on_started', 'out', E, D),
                   10: ('on_restarted', 'out', E, D)},
    'TestCurrentCheckpoint': {0: ('comparison', 'param', E, G), 1: ('checkpoint', 'param', E, D),
                              2: ('test', 'in', E, D), 3: ('on_true', 'out', E, D), 4: ('on_false', 'out', E, D)},
    'CheckpointFailure': {3: ('fail', 'in', E, D)},
    'TestTimeOfDay': {0: ('test', 'in', E, D), 1: ('from_hour', 'param', E, D), 2: ('to_hour', 'param', E, D),
                      3: ('on_true', 'out', E, D), 4: ('on_false', 'out', E, D)},
    'TestBuildingOwnership': {0: ('building', 'param', E, G), 1: ('family', 'param', E, G), 2: ('test', 'in', E, D),
                              3: ('on_true', 'out', E, D), 4: ('on_false', 'out', E, D)},
    'TestScore': {0: ('score', 'param', E, G), 2: ('comparison', 'param', E, G), 3: ('test', 'in', E, D),
                  4: ('on_true', 'out', E, D), 5: ('on_false', 'out', E, D)},
    'TestFamilySettings': {1: ('test', 'in', E, D), 2: ('on_true', 'out', E, G), 3: ('on_false', 'out', E, G)},
    'TestUpgrade': {2: ('test', 'in', E, D), 3: ('on_true', 'out', D, G), 4: ('on_false', 'out', E, G)},
    'TestBuildingState': {2: ('test', 'in', E, D), 3: ('on_true', 'out', D, G)},
    'MissionTimer': {0: ('start', 'in', E, D), 1: ('stop', 'in', E, D), 3: ('seconds', 'param', E, D),
                     7: ('on_expired', 'out', E, D)},
    'UIHudObjective': {0: ('show', 'in', E, D), 1: ('hide', 'in', E, D), 2: ('style', 'param', E, G),
                       3: ('text', 'param', E, G)},
    'UIHudTutorial': {0: ('show', 'in', E, D), 1: ('hide', 'in', E, D), 2: ('text', 'param', E, G)},
    'UIFanfare': {0: ('show', 'in', E, D), 1: ('text', 'param', E, G)},
    'PhoneMessage': {0: ('call', 'in', E, D), 1: ('cancel', 'in', E, G), 2: ('conversation', 'param', E, D),
                     9: ('on_complete', 'out', D, D)},
    'SpawnPoint': {0: ('spawn_set', 'link', D, G), 4: ('enable', 'in', E, G), 5: ('disable', 'in', E, G),
                   6: ('on_spawned', 'out', E, G)},
    'FuncSpawn': {0: ('spawn', 'in', E, D), 1: ('on_spawned', 'out', E, D), 2: ('template', 'link', D, G)},
    'LuaAudioScript': {0: ('flags', 'param', E, E), 1: ('start', 'in', E, E), 2: ('stop', 'in', E, E),
                       3: ('script', 'asset', E, E)},
    'TrinityGameSequence': {0: ('play', 'in', E, D), 1: ('on_end', 'out', E, D), 6: ('on_end_2', 'out', E, G),
                            7: ('sequence', 'asset', D, D)},
    'InGameVideoPlayer': {0: ('play', 'in', E, D), 1: ('movie', 'param', E, D), 2: ('folder', 'param', E, D),
                          3: ('on_end', 'out', E, D)},
    'ScriptedSequence': {0: ('start', 'in', E, D), 1: ('on_end', 'out', D, G), 2: ('on_end_2', 'out', D, G),
                         3: ('actor', 'link', D, G)},
    'HitMission': {11: ('on_complete', 'out', E, D), 12: ('on_fail', 'out', E, D)},
    'DynamicJob': {0: ('job', 'param', E, D), 1: ('accept', 'in', E, D), 4: ('on_cancelled', 'out', E, D),
                   5: ('on_completed', 'out', E, D), 7: ('reset', 'in', E, D)},
    'DynamicJobInvoker': {0: ('start', 'in', E, D)},
    'JobReward': {13: ('give', 'in', E, D), 14: ('on_given', 'out', E, D)},
    'GiveItemRelay': {0: ('give', 'in', E, D), 1: ('item', 'link', D, G)},
    'ChangeNPCSettings': {0: ('apply', 'in', E, D), 1: ('apply_2', 'in', E, G), 2: ('npc', 'link', D, G)},
    'ChangeDialog': {0: ('apply', 'in', E, D), 1: ('npc', 'link', D, G), 3: ('dialog', 'param', E, D)},
    'PlaySound': {1: ('play', 'in', E, D), 2: ('stop', 'in', E, D)},
    'PlayAudioStream': {0: ('play', 'in', E, D), 1: ('stop', 'in', E, D), 3: ('stream', 'asset', D, G)},
    'SetAudioMix': {0: ('apply', 'in', E, D), 1: ('remove', 'in', E, G)},
    'FadeScreenFx': {0: ('fade', 'in', E, D), 2: ('seconds', 'param', E, G), 3: ('colour', 'param', E, D)},
    'SetTimeOfDay': {0: ('apply', 'in', E, D), 3: ('hour', 'param', E, D)},
    'ExecuteScoreEvent': {0: ('execute', 'in', E, D), 1: ('event', 'param', E, G)},
    'ExecuteVFX': {0: ('play', 'in', E, D), 1: ('stop', 'in', E, D), 7: ('vfx', 'asset', D, D)},
    'PlayerTeleportRelay': {1: ('teleport', 'in', E, D), 2: ('on_arrived', 'out', D, D)},
    'SetInvulnerability': {0: ('npc', 'link', D, G), 1: ('enable', 'in', E, D), 2: ('disable', 'in', E, D)},
    'Door': {11: ('open', 'in', E, D), 12: ('open_2', 'in', E, G), 13: ('close', 'in', E, D),
             14: ('lock', 'in', E, D), 15: ('unlock', 'in', E, D), 18: ('on_opened', 'out', E, D)},
    # InterestingObject 0x7260C0 (reader), 0x7279B0 (message handler): 1 and
    # 2 are subscribed (+0x54, +0x5C); message 1 sets the disabled bit and
    # unregisters the object (0x727F30), message 2 clears it and registers
    # the object again (0x728070).
    'InterestingObject': {1: ('disable', 'in', E, E), 2: ('enable', 'in', E, E), 3: ('on_start', 'out', E, G),
                          4: ('on_finish', 'out', E, G)},
    # NodeController 0x9A6B00: both messages subscribed (0x4089B0, +0x94 and
    # +0x8C); names from the data (`*_set_to_red` / `*_set_to_green`,
    # `apollo_disable_graph` / `apollo_enable_graph`).
    'NodeController': {0: ('apply', 'in', E, G), 1: ('remove', 'in', E, G)},
    # EnableDisableStreams 0x812980: both messages subscribed (0x4089B0,
    # +0x50 and +0x58).
    'EnableDisableStreams': {0: ('activate', 'in', E, G), 1: ('deactivate', 'in', E, G)},
}

CATEGORY_RULES = [
    ('trigger', lambda n: n.startswith('Trigger') or n == 'EnterExitTrigger'),
    ('logic', lambda n: n in ('MultiManager', 'AndEventGate', 'OrEventGate', 'MessageRelay', 'ForwardSender',
                              'MissionTimer', 'ClassMessageFilter', 'PrioritizeEvents')
     or n.startswith('Variable') or n.startswith('Test') or n.startswith('Checkpoint')
     or n == 'ThresholdTrackingEventTransmitter'),
    ('ui', lambda n: n.startswith('UI') or n in ('PhoneMessage', 'DebugText', 'HudIndicatorControl')),
    ('audio', lambda n: 'Audio' in n or 'Sound' in n or 'Music' in n or n in ('LuaAudioScript', 'SetAudioMix')),
    ('spawn', lambda n: 'Spawn' in n or 'Requester' in n),
    ('job', lambda n: n in ('HitMission', 'DynamicJob', 'DynamicJobInvoker', 'JobReward')),
    ('cinematic', lambda n: n in ('TrinityGameSequence', 'ScriptedSequence', 'InGameVideoPlayer', 'FadeScreenFx')),
    ('relay', lambda n: n.endswith('Relay') or n.startswith('Change') or n.startswith('Set')
     or n.startswith('Execute')),
]


def category(name, has_position):
    for cat, rule in CATEGORY_RULES:
        if rule(name):
            return cat
    return 'object' if has_position else 'action'


def main(root, readers_path):
    kinds = load_kinds()
    names = load_names()
    readers = json.load(open(readers_path))
    cname = lambda h: names.get(h, '%08x' % h)  # noqa: E731
    objects = collections.Counter()
    placed = collections.Counter()
    chains = collections.defaultdict(collections.Counter)
    values = collections.defaultdict(list)
    attr_count = {}
    guids = set()
    corpus = list(load_corpus(root, kinds))
    for cls, guid, pks in corpus:
        guids.add(guid)
        objects[cls] += 1
        chains[cls][tuple((h, na) for h, na, _ in pks)] += 1
        placed[cls] += any(h == 0xB390B11A for h, _, _ in pks)
        for h, na, attrs in pks:
            attr_count[h] = na
            for i, v in attrs.items():
                values[(h, i)].append(v)

    def exe(h, i):
        return readers.get('%08x:%d' % (h, i), {})

    # Messages sent and heard by `exe` pins.
    sent, heard = collections.Counter(), collections.Counter()
    for (h, i), vs in values.items():
        e = exe(h, i)
        if kinds.get((h, i)) != 'string' or e.get('type') != 'eventid':
            continue
        side = heard if e['dir'] == 'listen' else sent
        for v in vs:
            side[v.lower()] += 1

    pins = {}
    stats = collections.Counter()
    for (h, i), vs in sorted(values.items()):
        kind = kinds.get((h, i))
        e = exe(h, i)
        pin, conf = 'param', None
        if kind == 'string':
            if e.get('type') == 'eventid':
                pin, conf = ('in' if e['dir'] == 'listen' else 'out'), 'exe'
            else:
                low = [v.lower() for v in vs]
                as_in = sum(1 for v in low if sent[v]) / len(low)
                as_out = sum(1 for v in low if heard[v]) / len(low)
                if max(as_in, as_out) >= MESSAGE_SHARE:
                    pin, conf = ('in' if as_in >= as_out else 'out'), 'data'
        elif kind == 'guid':
            objs = sum(1 for v in vs if v in guids)
            res = sum(1 for v in vs if len(set(v[k:k + 4] for k in range(0, 16, 4))) == 1)
            if objs * 2 >= len(vs):
                pin, conf = 'link', 'data'
            elif res * 2 >= len(vs):
                pin, conf = 'asset', 'data'
        pins[(h, i)] = [None, pin, conf, None, kind]
        stats[(pin, conf)] += 1

    for cls_name, table in MANUAL.items():
        h = sdbm_lower(cls_name)
        for i, (name, pin, conf, name_conf) in table.items():
            entry = pins.setdefault((h, i), [None, 'param', None, None, kinds.get((h, i))])
            entry[0] = name
            if pin is not None:
                entry[1], entry[2] = pin, conf
            entry[3] = name_conf

    out = sys.stdout
    out.write('# Pins of SimGroup classes for the logic graph of The Godfather II (PC).\n')
    out.write('# Generated by research/logic_schema.py from the game files and the decompiled\n')
    out.write('# attribute readers of godfather2.exe; see docs/logic-graph.md §2.\n#\n')
    out.write('# [packet.<class>] — attributes one class of the hierarchy adds (the packet\n')
    out.write('# of that class); an object has the packets of all its base classes.\n')
    out.write('# pin: in (message the object listens to), out (message it sends), link\n')
    out.write('# (GUID of another object), asset (resource reference), param.\n')
    out.write('# conf: exe (seen in the code of the reader), data (inferred from the game\n')
    out.write('# files), guess. name_conf: the same for the name; no name — not known.\n')
    out.write('# delay: index of the attribute holding the delay of an out pin, seconds.\n\n')
    out.write('[class]\n')
    out.write('# object class = "category"; categories: trigger, logic, ui, audio, spawn,\n')
    out.write('# job, cinematic, relay, action (no position), object (placed in the world)\n')
    for cls in sorted(objects, key=cname):
        out.write('%s = "%s"\n' % (quote(cname(cls)), category(cname(cls), placed[cls] > 0)))
    out.write('\n[layout]\n')
    out.write('# object class = packets of a new object, base class first: "class hash:attribute\n')
    out.write('# count" (the most common layout of the class in the game)\n')
    for cls in sorted(objects, key=cname):
        layout = chains[cls].most_common(1)[0][0]
        out.write('%s = [%s]\n' % (quote(cname(cls)), ', '.join('"%08x:%d"' % p for p in layout)))
    out.write('\n')
    by_class = collections.defaultdict(list)
    for (h, i), entry in pins.items():
        by_class[h].append((i, entry))
    for h in sorted(by_class, key=cname):
        rows = sorted(by_class[h])
        if all(e[1] == 'param' and e[0] is None for _, e in rows):
            continue
        out.write('[packet.%s]\n' % quote(cname(h)))
        out.write('hash = "%08x"\n' % h)
        out.write('attrs = [\n')
        for i, (name, pin, conf, name_conf, kind) in rows:
            if pin == 'param' and name is None:
                continue
            fields = ['i = %d' % i, 'pin = "%s"' % pin]
            if conf:
                fields.append('conf = "%s"' % conf)
            if name:
                base, _, delay = name.partition('+delay=')
                fields.append('name = "%s"' % base)
                if name_conf:
                    fields.append('name_conf = "%s"' % name_conf)
                if delay:
                    fields.append('delay = %s' % delay)
            if kind:
                fields.append('kind = "%s"' % kind)
            out.write('  { %s },\n' % ', '.join(fields))
        out.write(']\n\n')
    for k, v in sorted(stats.items(), key=str):
        print(k, v, file=sys.stderr)


def quote(name):
    return name if name.replace('_', '').isalnum() else '"%s"' % name


if __name__ == '__main__':
    main(sys.argv[1], sys.argv[2])

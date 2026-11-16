#!/usr/bin/env python3
"""Builds data/gf2/game_messages.tsv: every message name the SimGroups of the
game use, with who sends it and who hears it.

Usage: message_catalog.py <simgroup_dump_dir> [--mentions <label>=<dump_dir> ...]
           > data/gf2/game_messages.tsv

Inputs:

- the SimGroups of the game, dumped with `doctor-toolkit dump --type simgroup`;
- data/gf2/logic_schema.toml: which attribute of which class packet is a
  message heard (`in`) or sent (`out`);
- optionally, dumps of other resource types (`--mentions lua=<dir>`,
  `score=<dir>`, `seq=<dir>` for `trinity_seq_master`): every zero-terminated
  printable string of those files whose `sdbm(lower)` is the hash of a message
  of the catalog or of data/gf2/engine_messages.txt counts as a mention. A
  mention says the name occurs in that resource, not that the resource sends
  the message.

Output, one message per line, tab-separated, sorted by name:

    name  senders  listeners  scope  streams  mentions

- name: the most frequent spelling (the engine compares `sdbm(lower(name))`);
- senders, listeners: `Class.index:count` joined with `,`, `-` when none; the
  class is the packet class holding the attribute, as logic_schema.toml
  spells it;
- scope: the longest common directory prefix of the streams that use the
  message (the stream itself when there is one), `*` when they share none;
- streams: number of streams (one SimGroup per stream) using the message;
- mentions: `label:count` joined with `,`, `-` when none.

Messages that only the mentions name (not used by any SimGroup) are not
listed. The summary goes to stderr.
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


def sdbm_lower(s):
    h = 0
    for c in s.lower().encode('latin1'):
        h = (c + h * 65599) & 0xFFFFFFFF
    return h


def load_pins():
    """(packet class hash, attribute index) -> (class name, 'in' | 'out')."""
    schema = tomllib.load(open(os.path.join(DATA, 'logic_schema.toml'), 'rb'))
    pins = {}
    for name, packet in schema['packet'].items():
        h = int(packet['hash'], 16)
        for a in packet['attrs']:
            if a['pin'] in ('in', 'out'):
                pins[(h, a['i'])] = (name, a['pin'])
    return pins


def load_engine():
    out = {}
    for line in open(os.path.join(DATA, 'engine_messages.txt')):
        line = line.strip()
        if line and not line.startswith('#'):
            out[sdbm_lower(line)] = line
    return out


def uses(root, pins):
    """Yields (stream, class name, attribute index, 'in' | 'out', message)."""
    for dp, _, fns in sorted(os.walk(root)):
        stream = os.path.relpath(dp, root).replace(os.sep, '/')
        for fn in sorted(fns):
            d = open(os.path.join(dp, fn), 'rb').read()
            n, auxsz, strsz, tbl = (struct.unpack_from('<I', d, o)[0] for o in (0x18, 0x20, 0x24, 0x28))
            pk = tbl + 4 * n + auxsz + strsz
            offs = struct.unpack_from('<%dI' % n, d, tbl)
            units, _ = packets(d, pk, offs[0] if n else len(d))
            for o in offs:
                ng, npk = d[o + 0x1E], d[o + 0x1F]
                for k in range(ng, ng + npk):
                    h, _, _, slots, idx = units[rel(d, o + 0x2C + 4 * k)]
                    for q, i in zip(slots, idx):
                        pin = pins.get((h, i))
                        if pin is None:
                            continue
                        p = rel(d, q)
                        text = d[p:d.index(b'\0', p)].decode('latin1')
                        if text:
                            yield stream, pin[0], i, pin[1], text


def strings(data, least=3):
    start = None
    for i, b in enumerate(data):
        if 0x20 <= b < 0x7F:
            if start is None:
                start = i
        else:
            if start is not None and b == 0 and i - start >= least:
                yield data[start:i].decode('latin1')
            start = None


def common_scope(streams):
    streams = sorted(streams)
    if len(streams) == 1:
        return streams[0]
    parts = [s.split('/') for s in streams]
    prefix = []
    for column in zip(*parts):
        if len(set(column)) != 1:
            break
        prefix.append(column[0])
    return '/'.join(prefix) or '*'


def main(argv):
    root = argv[1]
    mentions_dirs = []
    rest = argv[2:]
    while rest:
        if rest[0] != '--mentions' or len(rest) < 2:
            sys.exit(__doc__)
        label, _, path = rest[1].partition('=')
        mentions_dirs.append((label, path))
        rest = rest[2:]

    pins = load_pins()
    engine = load_engine()
    spell = collections.defaultdict(collections.Counter)
    senders = collections.defaultdict(collections.Counter)
    listeners = collections.defaultdict(collections.Counter)
    streams = collections.defaultdict(set)
    sends = hears = 0
    for stream, cls, index, side, text in uses(root, pins):
        h = sdbm_lower(text)
        spell[h][text] += 1
        streams[h].add(stream)
        if side == 'out':
            senders[h]['%s.%d' % (cls, index)] += 1
            sends += 1
        else:
            listeners[h]['%s.%d' % (cls, index)] += 1
            hears += 1

    mentions = collections.defaultdict(collections.Counter)
    for label, path in mentions_dirs:
        for dp, _, fns in os.walk(path):
            for fn in fns:
                seen = set()
                for s in strings(open(os.path.join(dp, fn), 'rb').read()):
                    h = sdbm_lower(s)
                    if (h in spell or h in engine) and h not in seen:
                        seen.add(h)
                        mentions[h][label] += 1

    out = sys.stdout
    out.write('# Messages of the SimGroups of The Godfather II (PC 1.0.789.0): who sends and\n')
    out.write('# who hears each name. Generated by research/message_catalog.py from the game\n')
    out.write('# files and data/gf2/logic_schema.toml; do not edit by hand.\n')
    out.write('# Columns (tab-separated): name, senders, listeners, scope, streams, mentions.\n')
    out.write('# senders, listeners: Class.attribute:count; scope: common directory of the\n')
    out.write('# streams using the message; mentions: other resources naming it (label:files).\n')

    def cell(counter):
        return ','.join('%s:%d' % kv for kv in sorted(counter.items())) or '-'

    rows = []
    for h, spellings in spell.items():
        name = sorted(spellings.items(), key=lambda kv: (-kv[1], kv[0]))[0][0]
        rows.append((name.lower(), name, h))
    rows.sort()
    stats = collections.Counter()
    for _, name, h in rows:
        out.write('\t'.join([name, cell(senders[h]), cell(listeners[h]), common_scope(streams[h]),
                             str(len(streams[h])), cell(mentions[h])]) + '\n')
        s, l, e, m = bool(senders[h]), bool(listeners[h]), h in engine, bool(mentions[h])
        stats['messages'] += 1
        stats['engine names' if e else 'game names'] += 1
        if s and l:
            stats['sent and heard'] += 1
        elif l:
            stats['heard, engine name' if e else 'heard, never sent'] += 1
            if not e and m:
                stats['heard, never sent, mentioned elsewhere'] += 1
        else:
            stats['sent, engine name' if e else 'sent, never heard'] += 1
            if not e and m:
                stats['sent, never heard, mentioned elsewhere'] += 1
        for label in mentions[h]:
            stats['mentioned in ' + label] += 1
        stats['spelled in more than one case'] += len(spellings) > 1
    stats['sends'] = sends
    stats['subscriptions'] = hears
    stats['engine names not used by SimGroups'] = sum(1 for h in engine if h not in spell)
    for label in sorted({l for c in mentions.values() for l in c}):
        stats['engine names not used by SimGroups, mentioned in ' + label] = sum(
            1 for h in engine if h not in spell and mentions[h][label])
    # Different names with one hash would make the engine mix them up.
    lowered = collections.defaultdict(set)
    for h, spellings in spell.items():
        for s in spellings:
            lowered[h].add(s.lower())
    stats['hash collisions between names'] = sum(1 for v in lowered.values() if len(v) > 1)
    for k, v in sorted(stats.items()):
        sys.stderr.write('%-52s %d\n' % (k, v))


if __name__ == '__main__':
    if len(sys.argv) < 2:
        sys.exit(__doc__)
    main(sys.argv)

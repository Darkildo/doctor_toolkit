#!/usr/bin/env python3
"""Builds data/gf2/enums.toml: the value lists of The Godfather II (PC
1.0.789.0) — families, cities, venues, building types, weapons, crew
specialties, sound categories, emotions.

Usage: enum_values.py <simgroup_dump_dir> <auc.bin> <pad.bin> > data/gf2/enums.toml

Inputs, all made with `doctor-toolkit dump --type <type>`:

- the SimGroups of the game (`--type simgroup`);
- one `auc` resource (the two of the game are identical);
- one `pad` resource (the two of the game list the same names).

Where each list comes from:

- family: attribute 1 of every `Family` object (gf2/globals_global). The
  engine keeps `sdbm(lower(name))` (Family+0x54, lookup 0x008C74D0);
- city: attribute 0 of every `City` object; `Building` 3 and other attributes
  hold `sdbm(lower(name))` of it;
- venue: attribute 0 of every `Building` object (the venue id that
  `CMD_SetVenueID` attributes repeat), with its city (3), owner family (4)
  and type (1) as a note;
- building_type: the values of `Building` 1; their names are not known;
- weapon: attribute 0 of the `Item` packet of every object that has one
  (guns, thrown and melee weapons, pick-ups), with the item type (`Item` 3);
- crew_specialty: the bits tested by 0x009630E0 of godfather2.exe, which
  returns `$crew_specialty_<name>` for each (CREW_SPECIALTIES below);
- sound_category: the 49 records of the `auc` resource (name, parent group);
- emotion: the records of the `pad` resource (set `pedestrian`); each record
  holds `sdbm(lower(name))` before the name.

The script checks what it relies on (hashes of the `pad` records, sizes of
the `auc` tables) and stops when the data disagrees.
"""
import collections
import os
import struct
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from simgroup_survey import packets, rel  # noqa: E402

HERE = os.path.dirname(os.path.abspath(__file__))

# 0x009630E0: `if (flags & bit) return "$crew_specialty_<name>"`, in this order.
CREW_SPECIALTIES = [
    (0x04, 'demolition'),
    (0x08, 'arsonist'),
    (0x10, 'safe_cracker'),
    (0x20, 'engineer'),
    (0x40, 'medic'),
    (0x80, 'bruiser'),
]


def sdbm_lower(s):
    h = 0
    for c in s.lower().encode('latin1'):
        h = (c + h * 65599) & 0xFFFFFFFF
    return h


FAMILY, CITY, BUILDING, ITEM = (sdbm_lower(n) for n in ('Family', 'City', 'Building', 'Item'))
# (packet class, attribute) read as text; everything else as u32.
STRINGS = {(FAMILY, 1), (CITY, 0), (ITEM, 0), (ITEM, 31)}


def load_names():
    names = {}
    for line in open(os.path.join(HERE, '..', 'data', 'gf2', 'sim_classes.txt')):
        line = line.strip()
        if line and not line.startswith('#'):
            names[sdbm_lower(line)] = line
    return names


def objects(root):
    """Yields (stream, object class, {packet class: {index: value}})."""
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
                cls = struct.unpack_from('<I', d, o + 0x28)[0]
                found = {}
                for k in range(ng, ng + npk):
                    h, _, _, slots, idx = units[rel(d, o + 0x2C + 4 * k)]
                    if h not in (FAMILY, CITY, BUILDING, ITEM):
                        continue
                    attrs = {}
                    for q, i in zip(slots, idx):
                        if (h, i) in STRINGS:
                            p = rel(d, q)
                            attrs[i] = d[p:d.index(b'\0', p)].decode('latin1')
                        else:
                            attrs[i] = struct.unpack_from('<I', d, q)[0]
                    found[h] = attrs
                if found:
                    yield stream, cls, found


def cstr(d, at, size):
    return d[at:at + size].split(b'\0')[0].decode('latin1')


def sound_categories(path):
    d = open(path, 'rb').read()
    count = struct.unpack_from('<I', d, 0x10)[0]
    assert 0x20 + count * 100 <= len(d), 'auc: the category table does not fit'
    out = []
    for k in range(count):
        at = 0x20 + k * 100
        name, group = cstr(d, at, 64), cstr(d, at + 64, 36)
        assert name and group, 'auc: empty category record %d' % k
        out.append((name, group))
    return out


def emotions(path):
    d = open(path, 'rb').read()
    count, set_hash = struct.unpack_from('<II', d, 0)
    set_name = cstr(d, 0x18, 0x24)
    assert sdbm_lower(set_name) == set_hash, 'pad: the set hash is not sdbm(lower(name))'
    size = (len(d) - 0x3C) // count
    assert 0x3C + count * size == len(d), 'pad: records do not fill the resource'
    out = []
    for k in range(count):
        at = 0x3C + k * size
        name = cstr(d, at + 8, 0x30)
        assert struct.unpack_from('<I', d, at + 4)[0] == sdbm_lower(name), 'pad: bad hash of %s' % name
        out.append(name)
    return set_name, out


def q(s):
    return '"' + s.replace('\\', '\\\\').replace('"', '\\"') + '"'


def main(root, auc, pad):
    class_names = load_names()
    families, cities, buildings = [], [], []
    items = collections.defaultdict(collections.Counter)
    for stream, cls, found in objects(root):
        if cls == FAMILY:
            families.append(found[FAMILY][1])
        if cls == CITY:
            cities.append((found[CITY][0], stream))
        if cls == BUILDING:
            buildings.append((found[BUILDING], stream))
        if ITEM in found and found[ITEM].get(0):
            a = found[ITEM]
            items[a[0]][(class_names.get(cls, '%08x' % cls), a.get(3), a.get(31))] += 1
    assert len(set(f.lower() for f in families)) == len(families), 'two families share a name'
    family_by_hash = {sdbm_lower(f): f for f in families}
    city_by_hash = {sdbm_lower(c): c for c, _ in cities}

    out = sys.stdout
    w = out.write
    w('# Value lists of The Godfather II (PC 1.0.789.0). Generated by\n')
    w('# research/enum_values.py from the game files and godfather2.exe; do not edit\n')
    w('# by hand. The lists become Rust types (crates/doctor-toolkit/build.rs), Python\n')
    w('# stubs, JSON Schema, drop-down lists and notes of the knowledge base.\n#\n')
    w('# [<enum>] title, ru (title for the Russian notes), kind, source, source_ru (the\n')
    w('# source for the Russian notes), conf, values.\n')
    w('# kind: how an attribute stores a value — "hash" (sdbm(lower(name)) as u32),\n')
    w('# "name" (the text), "int" (the number `value`), "bits" (the bit mask `value`).\n')
    w('# conf: exe (seen in the code), data (read from the game files), guess.\n')
    w('# values: name (absent when not known), value, note (key=value facts: city,\n')
    w('# owner, type and stream of a venue; class, objects and item31 — the animation\n')
    w('# set — of an item; `-` when the game sets none).\n')

    def section(key, title, ru, kind, source, source_ru, conf, values):
        w('\n[%s]\ntitle = %s\nru = %s\nkind = %s\nsource = %s\nsource_ru = %s\nconf = %s\nvalues = [\n'
          % (key, q(title), q(ru), q(kind), q(source), q(source_ru), q(conf)))
        for v in values:
            fields = []
            if v.get('name') is not None:
                fields.append('name = ' + q(v['name']))
            if v.get('value') is not None:
                fields.append('value = %d' % v['value'])
            if v.get('note'):
                fields.append('note = ' + q(v['note']))
            w('  { ' + ', '.join(fields) + ' },\n')
        w(']\n')

    section('family', 'Families', 'Семьи', 'hash',
            'attribute 1 of the Family objects of gf2/globals_global; the engine keeps '
            'sdbm(lower(name)) at Family+0x54 (lookup 0x008C74D0)',
            'атрибут 1 объектов Family в gf2/globals_global; движок хранит sdbm(lower(имя)) в '
            'Family+0x54 (поиск 0x008C74D0)', 'data',
            [{'name': f} for f in families])

    section('city', 'Cities', 'Города', 'hash',
            'attribute 0 of the City objects; Building 3 and other attributes hold sdbm(lower(name))',
            'атрибут 0 объектов City; Building 3 и другие атрибуты хранят sdbm(lower(имя)); в примечании '
            '— поток объекта City',
            'data', [{'name': c, 'note': 'stream=' + s}
                     for c, s in sorted(cities, key=lambda x: (x[1].startswith('gf2/') is False, x[1]))])

    venues = collections.OrderedDict()
    types = collections.defaultdict(list)
    for a, stream in sorted(buildings, key=lambda b: (b[0].get(0, 0), b[1])):
        vid = a.get(0, 0)
        city = city_by_hash.get(a.get(3))
        owner = family_by_hash.get(a.get(4)) if 4 in a else None
        assert city is not None, 'Building %d: attribute 3 is not a city' % vid
        assert 4 not in a or owner is not None, 'Building %d: attribute 4 is not a family' % vid
        types[a.get(1)].append(vid)
        note = 'city=%s owner=%s type=%s stream=%s' % (city, owner or '-', a.get(1, '-'), stream)
        venues.setdefault(vid, []).append(note)
    section('venue', 'Venues (buildings)', 'Заведения (здания)', 'int',
            'attribute 0 of the Building objects (the id CMD_SetVenueID attributes repeat); '
            'note: city (Building 3), owner family in the data (Building 4), type (Building 1), stream, '
            'more_objects (other Building objects with the same id)',
            'атрибут 0 объектов Building (тот же номер повторяют атрибуты CMD_SetVenueID); в примечании — '
            'город `city` (Building 3), семья-владелец по данным `owner` (Building 4), тип `type` '
            '(Building 1), поток `stream`, `more_objects` — сколько ещё объектов Building с этим номером',
            'data', [{'value': vid, 'note': notes[0] + (' more_objects=%d' % (len(notes) - 1)
                                                         if len(notes) > 1 else '')}
                     for vid, notes in venues.items()])

    def ranges(ids):
        ids = sorted(set(ids))
        return 'buildings=%d ids=%d..%d' % (len(ids), ids[0], ids[-1])
    section('building_type', 'Building types', 'Типы зданий', 'int',
            'values of attribute 1 of the Building objects; the names of the types are not known',
            'значения атрибута 1 объектов Building; имена типов неизвестны; в примечании — число '
            'зданий `buildings` и границы их номеров `ids`',
            'data', [{'value': t, 'note': ranges(ids)} for t, ids in sorted((t, i) for t, i in types.items() if t is not None)])

    weapons = []
    for name, uses in sorted(items.items(), key=lambda kv: kv[0].lower()):
        classes = sorted({c for c, _, _ in uses})
        numbers = sorted({n for _, n, _ in uses if n is not None})
        anims = sorted({a for _, _, a in uses if a})
        note = 'class=%s objects=%d' % ('/'.join(classes), sum(uses.values()))
        if anims:
            note += ' item31=' + '/'.join(anims)
        weapons.append({'name': name, 'value': numbers[0] if len(numbers) == 1 else None, 'note': note})
    section('weapon', 'Weapons and items', 'Оружие и предметы', 'name',
            'attribute 0 of the Item packet of Gun, ThrowableItem and Item objects; value: Item 3 '
            '(item type number)',
            'атрибут 0 пакета Item у объектов Gun, ThrowableItem и Item; число — Item 3 (номер типа '
            'предмета); в примечании — класс объекта `class`, число объектов `objects`, набор анимаций '
            '`item31` (Item 31)',
            'data', weapons)

    section('crew_specialty', 'Crew specialties', 'Специализации команды', 'bits',
            'bits of the crew slot field +0x10 tested by 0x009630E0 of godfather2.exe, which returns '
            'the text key $crew_specialty_<name>',
            'биты поля +0x10 места команды, которые проверяет 0x009630E0 в godfather2.exe и возвращает '
            'ключ текста $crew_specialty_<имя>', 'exe',
            [{'name': n, 'value': b} for b, n in CREW_SPECIALTIES])

    section('sound_category', 'Sound categories', 'Категории звука', 'name',
            'records of the auc resource (gf2/globals_global, gf2/frontend): name[64], parent group[36]',
            'записи ресурса auc (gf2/globals_global, gf2/frontend): имя[64], родительская группа[36] '
            '(`group` в примечании)',
            'data', [{'name': n, 'note': 'group=' + g} for n, g in sound_categories(auc)])

    set_name, emo = emotions(pad)
    section('emotion', 'Emotions', 'Эмоции', 'hash',
            'records of the pad resource (set `%s`, gf2/globals_global); each record holds '
            'sdbm(lower(name))' % set_name,
            'записи ресурса pad (набор `%s`, gf2/globals_global); в каждой записи лежит '
            'sdbm(lower(имя))' % set_name, 'data', [{'name': n} for n in emo])


if __name__ == '__main__':
    if len(sys.argv) != 4:
        sys.exit(__doc__)
    main(*sys.argv[1:])

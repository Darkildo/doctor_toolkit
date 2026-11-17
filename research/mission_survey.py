#!/usr/bin/env python3
"""Surveys the story of The Godfather II in the SimGroups of the game.

Reads the JSON files of `doctor-toolkit logic-export GAME -o DIR --type simgroup`.

Usage:
    mission_survey.py DIR checkpoints          # every Checkpoint object: number, chapter, title, messages
    mission_survey.py DIR chapters             # chapters (attribute 3) with their checkpoints: the table of missions
    mission_survey.py DIR chapters-md          # the same as a Markdown table (docs/analysis/story.md §4)
    mission_survey.py DIR class NAME [N]       # objects of a class with the attributes of its own packet
    mission_survey.py DIR attrs NAME           # per attribute of a class: how many objects set it, examples
    mission_survey.py DIR paths                # the longest stream paths (see the path-length crash)

Output is tab-separated text; nothing of the game is written.
"""
import collections
import glob
import json
import os
import sys


def groups(root):
    for f in sorted(glob.glob(os.path.join(root, "**", "*.json"), recursive=True)):
        d = json.load(open(f))
        if d.get("type") != "simgroup" and "objects" not in d:
            continue
        yield os.path.dirname(os.path.relpath(f, root)), d


def own(o):
    """Attributes of the packet of the object's own class."""
    for p in o["packets"]:
        if p["class"] == o["class"]:
            return {a["index"]: a["value"] for a in p["attrs"]}
    return {}


def packet(o, name):
    for p in o["packets"]:
        if p.get("class_name") == name:
            return {a["index"]: a["value"] for a in p["attrs"]}
    return {}


def objects(root, name):
    for stream, d in groups(root):
        for i, o in enumerate(d["objects"]):
            if (o.get("class_name") or o["class"]) == name:
                yield stream, i, o


def checkpoints(root):
    rows = []
    for stream, i, o in objects(root, "Checkpoint"):
        a = own(o)
        rows.append((a.get(3, -1), a.get(1, -1), stream, i, a))
    return sorted(rows, key=lambda r: (r[0], r[1], r[2]))


def main():
    if len(sys.argv) < 3:
        sys.exit(__doc__)
    root, cmd = sys.argv[1], sys.argv[2]
    if cmd == "checkpoints":
        print("chapter\tnumber\tstream\tobject\ttitle\tchapter_title\tload\ton_loaded\ton_started\ton_restarted\tother")
        for ch, n, stream, i, a in checkpoints(root):
            other = {k: v for k, v in a.items() if k not in (0, 1, 2, 3, 7, 8, 9, 10)}
            print(f"{ch}\t{n}\t{stream}\t{i}\t{a.get(0, '')}\t{a.get(2, '')}\t{a.get(7, '')}\t{a.get(8, '')}\t{a.get(9, '')}\t{a.get(10, '')}\t{json.dumps(other, sort_keys=True)}")
    elif cmd == "chapters":
        by = collections.defaultdict(list)
        for ch, n, stream, i, a in checkpoints(root):
            by[ch].append((n, stream, a))
        print("chapter\tcheckpoints\tnumbers\tchapter_title\tstreams")
        total = 0
        for ch in sorted(by):
            rows = by[ch]
            total += len(rows)
            titles = sorted({a.get(2, "") for _, _, a in rows} - {""})
            streams = sorted({s for _, s, _ in rows})
            print(f"{ch}\t{len(rows)}\t{','.join(str(n) for n, _, _ in rows)}\t{' | '.join(titles)}\t{' | '.join(streams)}")
        print(f"# {len(by)} chapter(s), {total} checkpoint object(s)")
    elif cmd == "chapters-md":
        by = collections.defaultdict(list)
        for ch, n, stream, i, a in checkpoints(root):
            by[ch].append((n, stream, a))
        print("| Глава | Название (атрибут 2) | Чекпоинты (атрибут 1) | Поток |")
        print("|---:|---|---|---|")
        for ch in sorted(by):
            rows = by[ch]
            titles = sorted({a.get(2, "") for _, _, a in rows} - {""})
            streams = sorted({s.split("/")[-1] for _, s, _ in rows})
            print(f"| {ch} | {'; '.join(titles)} | {', '.join(str(n) for n, _, _ in rows)} | {', '.join(streams)} |")
    elif cmd == "class":
        limit = int(sys.argv[4]) if len(sys.argv) > 4 else 1 << 30
        for k, (stream, i, o) in enumerate(objects(root, sys.argv[3])):
            if k >= limit:
                break
            print(f"{stream}\t{i}\t{o['guid']}\t{json.dumps(own(o), sort_keys=True, ensure_ascii=False)}")
    elif cmd == "attrs":
        count = collections.Counter()
        examples = collections.defaultdict(collections.Counter)
        total = 0
        for stream, i, o in objects(root, sys.argv[3]):
            total += 1
            for k, v in own(o).items():
                count[k] += 1
                examples[k][json.dumps(v, ensure_ascii=False)] += 1
        print(f"# {total} object(s) of {sys.argv[3]}")
        for k in sorted(count):
            top = ", ".join(f"{v} x{c}" for v, c in examples[k].most_common(6))
            print(f"{k}\t{count[k]}\t{len(examples[k])} distinct\t{top}")
    elif cmd == "paths":
        rows = sorted({s for s, _ in groups(root)}, key=len, reverse=True)
        for s in rows[:25]:
            print(f"{len(s) + 4}\t{s}.str")
    else:
        sys.exit(__doc__)


if __name__ == "__main__":
    main()

#!/usr/bin/env python3
"""Replace text in every string table of the game, through the workspace.

    python replace_strings.py "/path/to/The Godfather II" "Corleone" "Korleone"
    python replace_strings.py GAME "Corleone" "Korleone" --stream gf2/text/dd7d7050 --apply

Each changed table is one undoable operation in the workspace
(`<game>/_doctor_toolkit/workspace`, or `--workspace DIR`). The game files are
only written with `--apply` (backups first, atomic writes, table-of-contents
sizes updated); undo with `doctor-toolkit workspace undo` or revert with
`doctor-toolkit workspace revert GAME --game`.
"""

import argparse
import sys

import doctor_toolkit as dt


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("game", help="game directory")
    parser.add_argument("old", help="text to replace (case-sensitive)")
    parser.add_argument("new", help="replacement")
    parser.add_argument("--stream", action="append", help="only this stream (repeatable)")
    parser.add_argument("--workspace", help="workspace directory")
    parser.add_argument("--apply", action="store_true", help="write the changes into the game")
    parser.add_argument("--dry-run", action="store_true", help="only count the matches")
    args = parser.parse_args()

    game = dt.Game(args.game)
    try:
        # A dry run does not create a workspace.
        ws = game.workspace(dir=args.workspace, create=not args.dry_run)
    except dt.InvalidArgumentError:
        ws = None
    tables = 0
    strings = 0
    for resource in game.resources("lh2", streams=args.stream):
        # The workspace's version: earlier edits are kept.
        table = ws.string_table(resource) if ws else resource.string_table()
        edits = {}
        for language in range(table.language_count):
            for hash, text in table.items(language):
                if args.old in text:
                    table.set(hash, text.replace(args.old, args.new), language)
                    edits[hash] = True
        if not edits:
            continue
        strings += len(edits)
        tables += 1
        print(f"{resource.key}: {len(edits)} string(s)")
        if not args.dry_run:
            ws.set_string_table(table)
    print(f"{strings} string(s) in {tables} table(s)")
    if args.dry_run or not tables:
        return 0
    if args.apply:
        report = ws.apply()
        for name, action, replaced in report["streams"]:
            print(f"{action} {name} ({replaced} resource(s))")
    else:
        print(f"recorded in {ws.dir}; run again with --apply, or `doctor-toolkit workspace apply`")
    return 0


if __name__ == "__main__":
    sys.exit(main())

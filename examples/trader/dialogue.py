#!/usr/bin/env python3
"""The trader's conversation: one line and a choice with four options.

    doctor-toolkit dialogue GAME 0823c6d1 --json > ticket.json
    python dialogue.py ticket.json trader.json
    doctor-toolkit dialogue-import GAME trader.json --raw trader.dlg

The tree is built over the ticket agent's dialogue of the game (its clip
groups and animations are kept, its nodes are replaced), so no data of the
game is stored here.

    #0 assign   variable 0x49 := true \\  what every dialogue of the game does
    #1 assign   variable 0x30 := true /  before its first line and choice
    #2 line     the trader speaks (the line the ticket agent says before her choice)
    #3 choice   option 0, option 1, option 2, time out, option 4
    #4 assign   variable 0x23 := true  -> the trader sends ChangeNPCSettings.3
    #5 assign   variable 0x27 := true  -> ChangeNPCSettings.4
    #6 assign   variable 0x24 := true  -> ChangeNPCSettings.5
    #7 end

After a purchase the tree goes back to the choice; "Nothing today" and the
time-out end the conversation. See docs/analysis/handing-out.md, section 7.
"""
import json
import struct
import sys

NONE = 0xFFFFFFFF
KIND_LINE, KIND_CHOICE, KIND_END, KIND_ASSIGN = 1, 0x22, 0x23, 0x24
LABELS = ["doctor_trader_a", "doctor_trader_b", "doctor_trader_c", "", "doctor_trader_leave"]
SECONDS_TO_ANSWER = 600.0


def main():
    source, target = sys.argv[1:3]
    after_purchase = sys.argv[3] if len(sys.argv) > 3 else "choice"
    with open(source, encoding="utf-8") as f:
        report = json.load(f)
    data = report["data"]
    # The ticket agent's choice and the line before it give the clip groups;
    # only the branches and the labels change.
    nodes = data["nodes"]
    at = next(i for i, n in enumerate(nodes) if n["kind"] == KIND_CHOICE)
    before = next(n for n in nodes if n["kind"] == KIND_LINE and at in n["children"])
    line = list(before["payload"])
    line[6] = line[7] = 0  # the first (and only) line of the tree
    menu = list(nodes[at]["payload"])
    menu[0] = struct.unpack("<I", struct.pack("<f", SECONDS_TO_ANSWER))[0]
    menu[11:16] = [0, 1, 2, NONE, 4]  # label slots; the time-out has none
    choice, end = 3, 7
    then = choice if after_purchase == "choice" else end
    data["nodes"] = [
        {"kind": KIND_ASSIGN, "children": [1], "payload": [0, 0, 0x49, 1]},
        {"kind": KIND_ASSIGN, "children": [2], "payload": [0, 0, 0x30, 1]},
        {"kind": KIND_LINE, "children": [choice], "payload": line},
        {"kind": KIND_CHOICE, "children": [4, 5, 6, end, end], "payload": menu},
        {"kind": KIND_ASSIGN, "children": [then], "payload": [0, 0, 0x23, 1]},
        {"kind": KIND_ASSIGN, "children": [then], "payload": [0, 0, 0x27, 1]},
        {"kind": KIND_ASSIGN, "children": [then], "payload": [0, 0, 0x24, 1]},
        {"kind": KIND_END, "children": [], "payload": None},
    ]
    data["labels"] = LABELS
    with open(target, "w", encoding="utf-8") as f:
        json.dump({"stream": report["stream"], "data": data}, f, indent=1)


if __name__ == "__main__":
    main()

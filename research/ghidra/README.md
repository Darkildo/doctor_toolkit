# Ghidra scripts for the logic schema

Headless scripts used to build `data/gf2/logic_schema.toml` and
`data/gf2/engine_messages.txt` from `godfather2.exe` (an analysed Ghidra 12
project of the exe; open a copy, the scripts do not save):

```sh
G=ghidra_12.2/support/analyzeHeadless
run() { $G proj gf2 -process -noanalysis -readOnly -scriptPath research/ghidra -postScript "$@"; }

# Per-class attribute readers: callers of the packet iterator 0x43AFF0 …
run Callers.java readers 0x43aff0
# … and the readers Ghidra has no function for: find the class hashes, make functions.
run FindConst.java consts 0x62ebe09b 0xdc24db08 …   # class hashes without a reader
run DecompAt.java at <addresses from consts/index.txt with "none">
python3 research/logic_readers.py readers readers.json at consts

# Messages named by engine code (callers of the message-table setters).
run Callers.java callers 0x408240 0x408980 0x4089b0

python3 research/logic_schema.py <simgroup dump dir> readers.json > data/gf2/logic_schema.toml
```

Functions of the message bus (see `docs/logic-graph.md` §2):

| Address | What |
|---|---|
| 0x4DAFD0 | `sdbm(lower(name))` |
| 0x408260 | intern a message hash in the global table `*0x1162380` (4096 buckets), store it in a handle |
| 0x408980 | handle from a string: hash + intern (no subscription) |
| 0x4089B0 | the same and subscribe the receiver (0x407E60) |
| 0x473EC0 | the same with a typed payload (`"BasePtrMsgData*"`) |
| 0x408240 | engine code: handle from a literal name |
| 0x408A00 | send: call every subscriber of the message |
| 0x4833A0 | class registration: hash, name, factory, size (called from 0x688AF0) |

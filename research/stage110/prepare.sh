#!/usr/bin/env bash
# prepare.sh: the shadows and the two mods of the stage 110 check. Each mod
# is made in a workspace of its own on the unmodified game:
#   - an object added to the SimGroup of the zone at the gate (a delta);
#   - a new zone: a stream with a SimGroup of its own and a cell of the
#     world grid of Havana (the cell is a delta of the city's shared `wgi`).
# Nothing is written into the game folder.
set -euo pipefail
here=$(dirname "$(realpath "$0")")
. "$here/env.sh"

rm -rf "$S/ws-a" "$S/ws-b" "$S"/*.zip "$S/config" "$S/cache"
for name in shadow written only-a; do
    bash "$ROOT/research/stage100/mkshadow.sh" "$name"
    # A game without an ASI loader of its own: the shadow gets one.
    if [[ -n ${LOADER:-} ]]; then cp "$LOADER" "$S/$name/dinput8.dll"; fi
    mkdir -p "$S/$name/scripts"
done
SH=$S/shadow

# make ID OBJECT OFFSET ZONE_OBJECT ZONE_OFFSET BOX_MIN BOX_MAX: the mod ID
# puts a copy of OBJECT of the zone at OFFSET from it, and makes a zone of
# its own with a copy of ZONE_OBJECT (where it is in the game) and a second
# one at ZONE_OFFSET from it. The GUIDs of the new objects go to guids.txt.
: >"$S/guids.txt"
guid_of() { sed -n 's/^object [0-9]* duplicated as [0-9]* (\([0-9a-f]*\)).*/\1/p'; }
make() {
    local id=$1 ws=$S/ws-$1 stream=gf2/doctor/s110$1 out
    out=$("$T" level duplicate "$SH" "$ZONE" "$GROUP" "$2" "--offset=$3" --workspace-dir "$ws")
    echo "$out" | head -1
    echo "$id-in-the-zone $(guid_of <<<"$out")" >>"$S/guids.txt"
    "$T" add-stream "$SH" "$stream" --parent gf2/cuba/local --workspace-dir "$ws" | head -1
    "$T" add-simgroup "$SH" "$stream" --copy-of "$ZONE#$GROUP" --object "$4" --workspace-dir "$ws" | head -1
    out=$("$T" level duplicate "$SH" "$stream" 0 0 "--offset=$5" --workspace-dir "$ws")
    echo "$out" | head -1
    echo "$id-in-its-new-zone $(guid_of <<<"$out")" >>"$S/guids.txt"
    "$T" world-grid add "$SH" gf2/cuba "$stream" "--min=$6" "--max=$7" --workspace-dir "$ws" | head -2
    "$T" mods build "$SH" --name "Bench ${id^^}" --id "bench-$id" --mod-version 1.0 \
        --author bench --dir "$ws" -o "$S/$id.zip"
    "$T" mods info "$S/$id.zip" --resources | grep -E "^format|delta|added|new stream|resource\(s\)"
}
# The objects of the new zones are `InterestingObject`s: an object with a
# model needs its model in its own stream or an ancestor, and a copy of a
# Destructible of the zone in a stream of its own crashed the game while it
# loaded (page fault at 004A3E5B) — with deltas and with whole resources.
# A: a copy of object 8 next to the player; a zone with copies of object 37.
make a 8 47,0,27 37 103.4,0,21.3 266,-5,356 306,15,396
# B: a copy of object 16 on the other side; a zone with copies of object 38.
make b 16 72,0,36 38 96.7,0,10.5 260,-5,346 300,15,386
cat "$S/guids.txt"

for name in shadow written; do
    "$T" mods install "$S/$name" "$S/a.zip" "$S/b.zip" | grep -v "^run"
done
"$T" mods install "$S/only-a" "$S/a.zip" | grep -v "^run"
echo "--- check and conflicts of the two mods:"
"$T" mods check "$SH"
"$T" mods conflicts "$SH"
echo "--- written by the mod manager into $S/written:"
"$T" mods apply "$S/written" | tail -4
"$T" mods status "$S/written" | head -8
ls "$S/written/_doctor_toolkit/mods/.folded" "$S/written/_doctor_toolkit/mods/.folded/payloads"

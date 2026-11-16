#!/usr/bin/env bash
# New streams that the game loads the way it loads its own zones (stage 84).
#
#   examples/zones/world-grid-zone.sh GAME_COPY WORKSPACE
#
# GAME_COPY is a copy of the game (or a shadow made by `tools/game-bench.sh
# shadow`); nothing is written into it: every change is recorded in WORKSPACE.
# Give the workspace to the game with `doctor-toolkit live install GAME_COPY
# --apply --workspace-dir WORKSPACE`, or write it with `workspace apply`.
#
# The three ways below were checked in the game (Havana, `-stream gf2 cuba`;
# docs/analysis/game-bench.md §33):
#
#   1. a cell of the world grid: loaded when the player comes near;
#   2. a child of a world zone: loaded and unloaded with that zone;
#   3. a named cell and a ZoneStreamEnable object: loaded while the player is
#      inside the box of the object.
set -euo pipefail
game=${1:?game copy}
ws=${2:?workspace directory}
t=${DOCTOR_TOOLKIT:-doctor-toolkit}

level=gf2/cuba                           # the root stream: it holds the world grids
local=gf2/cuba/local                     # loaded all the time in Havana
zone=gf2/cuba/local/van/high/terrain4    # a world zone of the game

# Every new stream needs content; here each gets a copy of a texture.
texture() { "$t" clone-resource "$game" "$zone" --index "$1" --to "$2" --workspace-dir "$ws"; }

# 1. A stream in the world grid. The game loads it within about 180 m of the
#    centre of the box (as far as the 25 MB budget of the high grid lasts)
#    and unloads it when the player leaves.
"$t" add-stream "$game" gf2/doctor/zone --parent "$local" --workspace-dir "$ws"
texture 2 gf2/doctor/zone
"$t" world-grid add "$game" "$level" gf2/doctor/zone \
    --min=-532,0,125 --max=-492,20,165 --workspace-dir "$ws"

# 2. A child stream of a world zone that is not disabled by default: the game
#    opens it with every load of the zone. No grid is changed.
"$t" add-stream "$game" gf2/doctor/kid --parent "$zone" --with-parent --auto-load \
    --workspace-dir "$ws"
texture 6 gf2/doctor/kid

# 3. A named cell that waits for a ZoneStreamEnable object. The object is a
#    copy of the game's `venue_055` one (SimGroup 714 of gf2/cuba/local,
#    object 83): a box of 76 x 32 x 112 m turned by 45 degrees.
"$t" add-stream "$game" gf2/doctor/named --parent "$local" --workspace-dir "$ws"
texture 4 gf2/doctor/named
"$t" world-grid add "$game" "$level" gf2/doctor/named \
    --min=-157,0,74 --max=-117,20,114 --name doctor_named --on-enable --workspace-dir "$ws"
added=$("$t" level add "$game" "$local" 714 83 --position=-136.9,8.6,93.7 --workspace-dir "$ws")
echo "$added"
object=$(sed -n 's/^added object \([0-9]*\).*/\1/p' <<<"$added")
"$t" simgroup-set "$game" "$local" 714 "$object" --set "ZoneStreamEnable.0=doctor_named" \
    --workspace-dir "$ws"

"$t" world-grid info "$game" "$level" --cells --workspace-dir "$ws" | grep -E '^#|doctor'

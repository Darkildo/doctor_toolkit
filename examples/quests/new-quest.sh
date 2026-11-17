#!/usr/bin/env bash
# A small new task put into the story of New York (stage 77).
#
#   examples/quests/new-quest.sh GAME_COPY WORKSPACE [MOD.zip]
#
# GAME_COPY is a copy of the game (or a shadow made by `tools/game-bench.sh
# shadow`); nothing is written into it: every change is recorded in WORKSPACE.
# With MOD.zip the workspace is built into a mod for the mod manager. Give the
# workspace to the game with `doctor-toolkit live install GAME_COPY --apply
# --workspace-dir WORKSPACE`, or write it with `workspace apply`.
#
# The task ("go to the marked place"), as the game's own chapters are made
# (docs/analysis/story.md §3 and §8):
#
#   start message ─> Checkpoint (number, chapter) ─> on_started ─┐
#                                    └> on_restarted ────────────┤
#                                                                v
#        UIHudObjective.show, UIHudMiniMapMarker.show, TriggerBox.enable
#   the player walks into the box ─> "reached" ─> UIHudObjective.hide,
#        marker off, VariableOperator (a saved variable = 1), UIFanfare,
#        autosave, and the message that starts what comes next
#
# Every object is a copy of an object of the game (`level add` with a
# template), put into the SimGroup of `gf2/queens`, where the game keeps the
# Checkpoint objects of New York. What was checked in the game and what was
# not: docs/analysis/game-bench.md §40.
set -euo pipefail
game=${1:?game copy}
ws=${2:?workspace directory}
mod=${3:-}
t=${DOCTOR_TOOLKIT:-doctor-toolkit}

# --- what to change for a task of your own --------------------------------
prefix=dq                       # every message of the task starts with it
number=${QUEST_NUMBER:-105}     # checkpoint number: between the neighbours in the story (here 104 and 109)
chapter=${QUEST_CHAPTER:-4}     # chapter number (attribute 3 of Checkpoint)
title="Doctor quest - go to the marked place"
place=${QUEST_PLACE:--500,6.6,-248}   # X,Y,Z of the place to reach (Y is up)
# What starts the task. Empty: a TriggerAuto sends the start message
# `start_delay` seconds after New York loads (for a look at the task from a
# quick start; in a real story every load would start it again). Otherwise the
# name of a message of the game, e.g. the `on_started` of the checkpoint before.
start_after=${QUEST_START_AFTER:-}
start_delay=${QUEST_START_DELAY:-30}
# What the task starts when it is done: the load message of the next
# checkpoint (e.g. checkpoint_109_load). Empty: nothing.
next=${QUEST_NEXT:-}
variable_hash=77d00001          # entry hash of the saved variable: any number the game does not use
# ---------------------------------------------------------------------------

level=gf2/queens
group=6                                                              # the SimGroup of the level stream
chapter10=gf2/queens/global/checkpoints/c10-takeover2ndracket/c10_common_folderstream   # templates, SimGroup 7
globals=gf2/globals_global                                           # template of the fanfare
# the SimGroup of the globals (resource 2955 of the unchanged game)
globals_group=$("$t" list "$game/$globals.str" | awk '$2 == "simgroup" { print $1; exit }')

# add TEMPLATE_STREAM TEMPLATE_GROUP TEMPLATE_OBJECT [--position=X,Y,Z] -> index of the new object
add() {
    local stream=$1 resource=$2 object=$3 out
    shift 3
    out=$("$t" level add "$game" "$level" "$group" "$object" \
        --template-stream "$stream" --template-resource "$resource" --force "$@" --workspace-dir "$ws")
    sed -n 's/^added object \([0-9]*\).*/\1/p' <<<"$out"
}
# set OBJECT Class.attr=value ...
set_attrs() {
    local object=$1 args=()
    shift
    for a in "$@"; do args+=(--set "$a"); done
    "$t" simgroup-set "$game" "$level" "$group" "$object" "${args[@]}" --workspace-dir "$ws" >/dev/null
}
hash_of() { "$t" hash "$1" | sed -n 's/.*sdbm(lower) \([0-9a-f]*\).*/\1/p'; }

# 1. Texts: the objective and the fanfare. SimGroup attributes hold the hash of the name.
"$t" text-add "$game" "${prefix}_objective" "Go to the marked place" \
    --lang ru="Дойдите до отмеченного места" --workspace-dir "$ws" >/dev/null
"$t" text-add "$game" "${prefix}_complete" "Doctor quest complete" \
    --lang ru="Задание доктора выполнено" --workspace-dir "$ws" >/dev/null
objective_text=$(hash_of "${prefix}_objective")
complete_text=$(hash_of "${prefix}_complete")

# 2. A variable that goes into the saves (kind 1) in the dictionary of the level.
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
"$t" logic-export "$game" -o "$tmp" --type variabledictionary --stream "$level" >/dev/null
dict=$(ls "$tmp/$level"/*_variabledictionary_*.json)
python3 - "$dict" "${prefix}_done" "$variable_hash" <<'EOF'
import json, sys
path, name, entry = sys.argv[1:]
doc = json.load(open(path))
if not any(v["name"] == name for v in doc["variables"]):
    doc["variables"].append({"name": name, "hash": entry, "kind": 1, "value": 0})
json.dump(doc, open(path, "w"), indent=1)
EOF
"$t" logic-import "$game" "$dict" --workspace-dir "$ws" >/dev/null

# 3. The checkpoint: a copy of checkpoint 325. No stream set (attribute 5) and
#    no teleport relays (11-13): the task needs no streams of its own.
checkpoint=$(add "$level" "$group" 115)
set_attrs "$checkpoint" "Checkpoint.0=$title" "Checkpoint.1=$number" "Checkpoint.2=" \
    "Checkpoint.3=$chapter" "Checkpoint.5=" "Checkpoint.11=" "Checkpoint.12=" "Checkpoint.13=" \
    "Checkpoint.7=${prefix}_start" "Checkpoint.8=${prefix}_loaded" \
    "Checkpoint.9=${prefix}_started" "Checkpoint.10=${prefix}_restarted"

# 4. The start.
if [[ -z $start_after ]]; then
    auto=$(add "$level" "$group" 256)
    set_attrs "$auto" "TriggerAuto.0=${prefix}_start" "TriggerAuto.1=$start_delay"
else
    relay=$(add "$chapter10" 7 4)
    set_attrs "$relay" "MultiManager.0=$start_after" "MultiManager.1=1" "MultiManager.2=" \
        "MultiManager.3=${prefix}_start" "MultiManager.4=1" "MultiManager.5="
fi

# 5. Started or restarted (after a death or a loaded save) -> ready, and an autosave.
for from in started restarted; do
    ready=$(add "$chapter10" 7 4)
    set_attrs "$ready" "MultiManager.0=${prefix}_$from" "MultiManager.1=-1" "MultiManager.2=" \
        "MultiManager.3=${prefix}_ready" "MultiManager.4=2" "MultiManager.5="
done
save=$(add "$chapter10" 7 4)
set_attrs "$save" "MultiManager.0=${prefix}_started" "MultiManager.1=-1" "MultiManager.2=" \
    "MultiManager.3=cp_do_autosave_now" "MultiManager.4=4" "MultiManager.5="

# 6. The objective on the screen, the marker on the map, the box to walk into.
objective=$(add "$chapter10" 7 3)
set_attrs "$objective" "UIHudObjective.0=${prefix}_ready" "UIHudObjective.1=${prefix}_reached" \
    "UIHudObjective.3=$objective_text"
marker=$(add "$chapter10" 7 0 --position="$place")
set_attrs "$marker" "UIHudMiniMapMarker.9=${prefix}_ready" "UIHudMiniMapMarker.11=${prefix}_reached"
box=$(add "$chapter10" 7 16 --position="$place")
"$t" simgroup-set "$game" "$level" "$group" "$box" --scale 6,4,6 \
    --set "TriggerBase.1=${prefix}_ready" --set "TriggerBase.4=${prefix}_reached" \
    --set "TriggerBase.6=1" --set "TriggerBase.8=" --workspace-dir "$ws" >/dev/null

# 7. Reached: the variable, the fanfare, an autosave, what comes next.
done_var=$(add "$level" "$group" 201)
set_attrs "$done_var" "VariableOperator.0=${prefix}_reached" "VariableOperator.2=$variable_hash" \
    "VariableOperator.3=2" "VariableOperator.4=1"
fanfare=$(add "$globals" "$globals_group" 4372)
set_attrs "$fanfare" "UIFanfare.0=${prefix}_reached" "UIFanfare.1=$complete_text"
finish=$(add "$chapter10" 7 4)
set_attrs "$finish" "MultiManager.0=${prefix}_reached" "MultiManager.1=1" "MultiManager.2=" \
    "MultiManager.3=cp_do_autosave_now" "MultiManager.4=3" \
    "MultiManager.5=$next" "MultiManager.6=5"

echo "checkpoint $number (chapter $chapter): object $checkpoint of $level#$group; objective $objective, marker $marker, box $box"

if [[ -n $mod ]]; then
    "$t" mods build "$game" -o "$mod" --name "Doctor quest" --id doctor-quest --mod-version 1.0 \
        --description "A small new task in New York: example of doctor_toolkit (stage 77)" \
        --dir "$ws"
fi

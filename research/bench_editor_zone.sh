#!/usr/bin/env bash
# Bench check of a zone made entirely in the editor window (stage 103,
# docs/analysis/game-bench.md §47): a new stream with a SimGroup and a cell
# of the world grid of Havana, models imported with collision, props cloned
# from a neighbouring zone, objects placed in the level view — every step is
# an action of doctor-toolkit-gui run hands-free on its own Xvfb, nothing is
# written into the game while the zone is made. The mod built from it is
# then installed by the mod manager on a clean shadow and walked in the game.
#
#   research/bench_editor_zone.sh build GAME WORK MODELS
#       WORK/sh: a shadow of GAME for authoring (keep WORK short: "folder +
#       1 + stream path" must stay under 256 characters). MODELS: a folder
#       with models.json (`models`: name -> file, position, yaw; `extra`:
#       more places of a model; `centre`, `half`) and the model files it
#       names. Ends with WORK/editor-yard.zip and pictures of the editor in
#       WORK/gui.
#   research/bench_editor_zone.sh check GAME WORK
#       WORK/ts: a clean shadow; `mods install`, `mods check`, `mods apply`
#       of the ZIP, one session of the game in Havana (run it under the lock
#       of the bench), then `mods unapply`, `mods uninstall`, `verify` and a
#       comparison of the shadow with its state before the mod. The log of
#       the session is WORK/out/run.log.
#
# Environment: BENCH_PREFIX (default …/wine-gf2-bench-p103a), PORT (47931),
# DOCTOR_LIVE_MODULE (default WORK/doctor_live.asi, built when missing).
set -uo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
t=${DOCTOR_TOOLKIT:-$root/target/debug/doctor-toolkit}
gui=${GUI:-$root/target/debug/doctor-toolkit-gui}
bench=$root/tools/game-bench.sh
port=${PORT:-47931}
ZONE=gf2/testmods/editor_yard
LEVEL=gf2/cuba
PARENT=gf2/cuba/local
# A plain Destructible of the zone at the airport gate: the template of the
# objects (its class and logic). A static model with Havok collision of the
# always loaded gf2/cuba/local: the template of the imported model sets.
TEMPLATE_ZONE=gf2/cuba/local/yuri/medium/zone4
TEMPLATE_OBJECT=4
MODEL_TEMPLATE=336c8dd3
# Props of the zone's own set, cloned into the new stream with everything
# they use in their streams: name, metamodel key, x, z, yaw of each place.
PROPS=(
    "crate gf2/cuba/local/yuri/high/terrain1:metamodel:f7e100fb 279.6,405.6,0 280.8,405.6,15 286.2,393.4,30"
    "bin gf2/cuba/local/yuri/high/terrain2:metamodel:90870f88 284.6,392.2,0 287.4,403.6,0"
    "fence gf2/cuba/local/yuri/high/terrain7:metamodel:59aa9c75 273.0,397.0,0 277.0,397.0,0"
)

cmd=${1:?usage: bench_editor_zone.sh build|check GAME WORK [MODELS]}
game=${2:?the game folder}
work=${3:?the working folder}
export DOCTOR_TOOLKIT_CONFIG_DIR=$work/cfg DOCTOR_TOOLKIT_CACHE_DIR=$work/cache
export DOCTOR_TOOLKIT_LOG_DIR=$work/log XDG_CACHE_HOME=$work/cache
export DOCTOR_TOOLKIT_NO_DETECT=1
mkdir -p "$work/cfg" "$work/gui" "$work/log"
unset WAYLAND_DISPLAY DBUS_SESSION_BUS_ADDRESS DISPLAY

say() { echo "=== $*"; }
shadow() {
    "$bench" shadow "$game" "$1" | tail -1
    # Copies of the game folder's own modules keep the bench from starting.
    rm -f "$1/d3d9.dll" "$1"/scripts/doctor_live.* "$1"/scripts/GF2ASI.*
}
# One run of the editor on its own Xvfb that ends with a picture; a failed
# action ends the script.
editor() {
    local name=$1
    shift
    local code=0
    env DOCTOR_TOOLKIT_GAME_PROCESS=no-such-game.exe DOCTOR_TOOLKIT_GUI_SHOT="$work/gui/$name.png" "$@" \
        timeout "${GUI_TIMEOUT:-900}" xvfb-run -a -s "-screen 0 1600x900x24" "$gui" "$sh" >"$work/gui/$name.log" 2>&1 || code=$?
    grep -E '^(zone|level|DOCTOR_TOOLKIT_GUI_WIZARD):' "$work/gui/$name.log" | sed 's/^/    /'
    if ((code != 0)) || [[ ! -s $work/gui/$name.png ]]; then
        echo "the editor failed at $name (exit code $code), see $work/gui/$name.log" >&2
        tail -5 "$work/gui/$name.log" >&2
        exit 1
    fi
    echo "    editor: $name done"
}
# The metamodels the workspace has added to the new stream.
models_in_zone() { "$t" workspace status "$sh" 2>/dev/null | grep -o "$ZONE:metamodel:[0-9a-f]*" | sed 's/.*://' | sort -u; }
# Index of a resource of a stream by type and id.
index_of() { "$t" list "$sh/$1.str" | awk -v ty="$2" -v id="$3" '$2 == ty && $5 == id { print $1; exit }'; }
# Indices of a resource and of everything it uses in its own stream.
closure() {
    python3 -I - "$t" "$sh" "$1" <<'EOF'
import subprocess, sys
toolkit, game, key = sys.argv[1:4]
stream = key.split(":")[0]
def uses(k):
    text = subprocess.run([toolkit, "refs", game, "--limit", "0", k], capture_output=True, text=True).stdout
    out, inside = [], False
    for line in text.splitlines():
        if line.startswith("uses"):
            inside = True
        elif not line.startswith("    "):
            inside = False
        elif inside and " -> " in line:
            out.append(line.split(" -> ", 1)[1].split()[0])
    return out
seen, todo = [], [key]
while todo:
    k = todo.pop()
    if k in seen or k.split(":")[0] != stream:
        continue
    seen.append(k)
    todo += uses(k)
wanted = {(k.split(":")[1], k.split(":")[2]) for k in seen}
text = subprocess.run([toolkit, "list", f"{game}/{stream}.str"], capture_output=True, text=True).stdout
found = [c[0] for c in (l.split() for l in text.splitlines()) if len(c) > 4 and c[0].isdigit() and (c[1], c[4]) in wanted]
print("+".join(found))
EOF
}

build() {
    local models=${1:?the folder of the models}
    sh=$work/sh
    rm -rf "$sh" "$work/gui" "$work/editor-yard.zip"
    mkdir -p "$work/gui"
    say "authoring shadow"
    shadow "$sh"
    # The editor asks about the fingerprints of the game on its first run.
    printf '[games."%s"]\nskip_first_run = true\n' "$sh" >"$work/cfg/settings.toml"
    local box
    box=$(python3 -I -c '
import json, sys
m = json.load(open(sys.argv[1] + "/models.json"))
c, (hx, hz) = m["centre"], m["half"]
print(f"min={c[0]-hx}/{c[1]}/{c[2]-hz},max={c[0]+hx}/{c[1]+6.0}/{c[2]+hz}")' "$models")

    say "1. the New zone wizard: stream, SimGroup, cell of the world grid"
    editor 1-wizard DOCTOR_TOOLKIT_GUI_WIZARD="zone,name=$ZONE,parent=$PARENT,level=$LEVEL,$box,step=3"

    say "2. Import model (Into stream, Collision): the generated models"
    local template
    template="$PARENT#$(index_of "$PARENT" metamodel "$MODEL_TEMPLATE")"
    declare -A id
    local before after name file
    while read -r name file; do
        before=$(models_in_zone)
        editor "2-import-$name" DOCTOR_TOOLKIT_GUI_ZONE="import:file=$file,template=$template,name=$name,stream=$ZONE,collision=mesh;form=stream"
        after=$(models_in_zone)
        id[$name]=$(comm -13 <(echo "$before") <(echo "$after") | head -1)
        echo "    $name: metamodel ${id[$name]}"
    done < <(python3 -I -c '
import json, sys
for name, m in json.load(open(sys.argv[1] + "/models.json"))["models"].items():
    print(name.replace("tm_", "ed_"), m["file"])' "$models")

    say "3. Clone resources: props of the neighbouring zones with what they use"
    local prop key places
    declare -A places_of
    for entry in "${PROPS[@]}"; do
        read -r prop key places <<<"$entry"
        places_of[$prop]=$places
        before=$(models_in_zone)
        editor "3-clone-$prop" DOCTOR_TOOLKIT_GUI_ZONE="clone:stream=${key%%:*},index=$(closure "$key"),to=$ZONE;form=clone"
        after=$(models_in_zone)
        id[$prop]=$(comm -13 <(echo "$before") <(echo "$after") | head -1)
        echo "    $prop: metamodel ${id[$prop]}"
    done
    for name in "${!id[@]}"; do
        [[ -n ${id[$name]} ]] || { echo "no metamodel of $name in $ZONE" >&2; exit 1; }
    done

    say "4. the level view: objects added to the new zone and placed"
    # The template object of the neighbouring zone is selected; the first
    # object goes into the SimGroup of the new zone (Add to), the others are
    # added from the one placed before; `place` is what the Position and
    # Rotation fields of Details do.
    local acts="select=0:$TEMPLATE_OBJECT" first=":into=$ZONE" x y z yaw model
    while read -r model x y z yaw; do
        acts+=";act=add:${id[$model]}$first:at=$x/$y/$z;act=place:$x,$y,$z,$yaw"
        first=""
    done < <(python3 -I -c '
import json, sys
m = json.load(open(sys.argv[1] + "/models.json"))
rows = [(n, v["position"], v["yaw"]) for n, v in m["models"].items()]
rows += [(e["model"], e["position"], e["yaw"]) for e in m["extra"]]
for n, p, yaw in rows:
    print(n.replace("tm_", "ed_"), *p, yaw)' "$models")
    local place
    for prop in "${!places_of[@]}"; do
        for place in ${places_of[$prop]}; do
            IFS=, read -r x z yaw <<<"$place"
            acts+=";act=add:${id[$prop]}:at=$x/0.1/$z;act=place:$x,0.1,$z,$yaw"
        done
    done
    editor 4-level DOCTOR_TOOLKIT_GUI_LEVEL="$TEMPLATE_ZONE+$ZONE;$acts;eye=283.9,10,378;look=282,0.5,401" DOCTOR_TOOLKIT_GUI_SHOT_FRAMES=200
    editor 5-zone-alone DOCTOR_TOOLKIT_GUI_LEVEL="$ZONE;eye=283.9,12,376;look=282,0.5,401" DOCTOR_TOOLKIT_GUI_SHOT_FRAMES=200

    say "5. what was made, before anything is written into the game"
    "$t" workspace log "$sh" | tail -n +2 | wc -l | sed 's/$/ operation(s) in the workspace/'
    "$t" level list "$sh" gf2/testmods
    "$t" level objects "$sh" "$ZONE"
    "$t" check-refs "$sh" "$ZONE" | tail -3
    "$t" world-grid info "$sh" "$LEVEL" --cells | grep -c editor_yard | sed 's/$/ cell(s) of the zone in the world grid/'
    if [[ -e $sh/$ZONE.str ]]; then echo "THE GAME FOLDER HAS $ZONE.str"; else echo "the shadow has no $ZONE.str: nothing was written"; fi
    "$bench" verify "$game" "$sh" | tail -1

    say "6. Package a mod (the wizard)"
    editor 6-package DOCTOR_TOOLKIT_GUI_WIZARD="package,name=Editor yard,author=doctor_toolkit bench,description=A yard made in the editor window,out=$work/editor-yard.zip,step=4"
    [[ -s $work/editor-yard.zip ]] || { echo "the package wizard wrote no file" >&2; exit 1; }
    "$t" mods info "$work/editor-yard.zip" | head -12
}

# ------------------------------------------------------------------ check

L() { "$t" live "$@" --game "$ts" --port "$port" --timeout 120; }
D() { "$bench" do "$out" "$@" >>"$out/run.log" 2>&1; }
log() { echo "[$(date +%H:%M:%S)] $*" | tee -a "$out/run.log"; }
frame() { L status 2>/dev/null | sed -n 's/^main loop: GameMainLoop, frame \([0-9]*\).*/\1/p'; }
frames() {
    local a b
    a=$(frame)
    for _ in $(seq 1 240); do
        b=$(frame)
        [[ $((${b:-0} - ${a:-0})) -ge $1 ]] && return
        sleep 2
    done
}
gtime() { L status 2>/dev/null | sed -n 's/^game time: \([0-9]*\).*/\1/p'; }
gwait() {
    local a b
    a=$(gtime)
    for _ in $(seq 1 400); do
        b=$(gtime)
        [[ $((${b:-0} - ${a:-0})) -ge $1 ]] && return
        sleep 1
    done
}
tp() {
    "$t" live teleport --yes --game "$ts" --port "$port" --timeout 120 ${4:+--yaw=$4} -- "$1,$2,$3" >>"$out/run.log" 2>&1
    frames "${5:-6}"
}
yard() { echo "$1: yard streams loaded: $(L streams 2>/dev/null | grep -c editor_yard), objects: $(L objects --all --stream editor_yard 2>/dev/null | grep -c Destructible)" | tee -a "$out/run.log"; }
where() { echo "$1: $(L player 2>&1 | tr '\n' ' ')" | tee -a "$out/run.log"; }
crashed() {
    grep -q "Unhandled page fault\|Unhandled exception" "$out/wine.log" 2>/dev/null ||
        grep -q "status: exited" "$out/timeline.txt" 2>/dev/null
}

session() {
    export SERVER=xvfb DXVK=1 SIZE=1280x720 WINEDLLOVERRIDES=dinput8=n,b
    export BENCH_PREFIX=${BENCH_PREFIX:-$HOME/.local/share/doctor_toolkit/wine-gf2-bench-p103a}
    export SESSION_MAX=${SESSION_MAX:-2400}
    export DOCTOR_LIVE_MODULE=${DOCTOR_LIVE_MODULE:-$work/doctor_live.asi}
    [[ -f $DOCTOR_LIVE_MODULE ]] || "$root/tools/build-live-module.sh" "$DOCTOR_LIVE_MODULE" >/dev/null 2>&1
    rm -rf "$out"
    mkdir -p "$out"
    cd "$root" || exit 1
    stop() {
        log "stopping"
        "$bench" stop "$out" >>"$out/run.log" 2>&1
        "$t" live uninstall "$ts" >>"$out/run.log" 2>&1
        echo "wine.log exception lines: $(grep -ci 'Unhandled exception\|Unhandled page fault' "$out/wine.log" 2>/dev/null)" | tee -a "$out/run.log"
    }
    trap stop RETURN
    log "install the live module, start in Havana"
    "$t" live install "$ts" --writes --port "$port" "--cmd-txt=-windowed -resolution 1280 720 -stream gf2 cuba" >>"$out/run.log" 2>&1 ||
        { log "live install failed"; return 1; }
    "$bench" start "$ts" "$out" >>"$out/run.log" 2>&1 || { log "start failed"; return 1; }
    local ok=0 i j f
    for i in $(seq 1 240); do
        sleep 5
        if crashed; then
            log "the game crashed while loading"
            return 3
        fi
        if L streams >"$out/streams0.txt" 2>/dev/null && [[ $(grep -c cuba "$out/streams0.txt") -gt 20 ]]; then
            ok=1
            break
        fi
    done
    ((ok == 1)) || { log "the level did not load in time"; return 2; }
    log "level loaded after $((i * 5)) s"
    for j in $(seq 1 180); do
        f=$(frame)
        [[ ${f:-0} -ge 60 ]] && break
        sleep 5
    done
    for _ in $(seq 1 400); do
        [[ $(gtime) -ge 12 ]] 2>/dev/null && break
        sleep 1
    done
    L status | tee -a "$out/run.log"
    where spawn
    yard "at the spawn (28 m from the centre of the cell)"
    # The yard from outside (the wall with the gate faces the spawn), inside.
    tp 282 0 381 0 6; gwait 3
    D shot 01_gate_from_outside
    tp 282 1.0 396 0 6; gwait 3
    where "inside (the top of the slab is Y 0.10)"
    D shot 02_inside
    tp 285 1.0 394 -50 6; gwait 3
    D shot 03_props_left
    tp 279 1.0 394 50 6; gwait 3
    D shot 04_right
    # Walls: from the centre a run of 6 s in four directions; the inner faces
    # of the walls are x 270.4 / 293.6 and z 391.4 / 408.6, the gate is south.
    local dir
    for dir in "0 north(+Z)" "90 east" "180 south(gate)" "-90 west"; do
        set -- $dir
        tp 282 0.6 400 "$1" 6; gwait 1
        D hold w 6
        gwait 1
        where "run yaw $1 $2"
    done
    D shot 05_at_a_wall
    # A control: the same run outside the yard.
    tp 282 0.6 370 180 6; gwait 1
    D hold w 6
    gwait 1
    where "control run outside, yaw 180"
    # Walking in through the gate: the slab is a step of 0.1 m.
    tp 282 0.2 386 0 6; gwait 1
    D hold w 4
    gwait 1
    where "walked in through the gate"
    # Leaving: the palace is 660 m away; and back.
    tp -262 6 -3 0 6; gwait 25
    yard "at the palace (660 m)"
    where palace
    D shot 06_far_away
    tp 282 1.0 396 0 6; gwait 20
    yard "back in the yard"
    where back
    D shot 07_back
    L status | tee -a "$out/run.log"
    if crashed; then log "THE GAME CRASHED"; fi
}

state() {
    (cd "$ts" && find . -path ./.game-bench -prune -o -path ./_doctor_toolkit -prune -o -name '*.dxvk-cache' -prune -o \
        \( -type f -o -type l \) -print0 | sort -z | xargs -0 sha256sum | sha256sum | cut -c1-16)
}

check() {
    ts=$work/ts
    out=$work/out
    local zip=$work/editor-yard.zip before after
    [[ -s $zip ]] || { echo "no $zip: run build first" >&2; exit 1; }
    say "clean shadow"
    rm -rf "$ts"
    shadow "$ts"
    before=$(state)
    say "the mod manager: install, check, apply"
    "$t" mods install "$ts" "$zip" 2>&1 | head -2
    "$t" mods check "$ts" 2>&1 | tail -1
    "$t" mods apply "$ts" 2>&1 | tail -1
    "$t" mods status "$ts" 2>&1 | tail -1
    "$t" level objects "$ts" "$ZONE" 2>/dev/null | wc -l | sed 's/$/ object(s) of the zone in the game files/'
    say "the game"
    session || echo "session failed: $?"
    trap - RETURN
    say "remove"
    "$t" mods unapply "$ts" 2>&1 | tail -1
    "$t" mods uninstall "$ts" --all 2>&1 | tail -1
    "$bench" verify "$game" "$ts" | tail -1
    after=$(state)
    if [[ $before == "$after" ]]; then
        echo "the shadow is as before the mod (content of every file)"
    else
        echo "THE SHADOW DIFFERS from its state before the mod"
    fi
}

case $cmd in
    build) build "${4:-}" ;;
    check) check ;;
    *) echo "unknown command $cmd" >&2; exit 1 ;;
esac

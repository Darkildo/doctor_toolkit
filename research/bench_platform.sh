#!/usr/bin/env bash
# Bench check of the platform module and the example native mod (stage 80,
# docs/analysis/game-bench.md §43).
#
#   research/bench_platform.sh setup GAME SHADOW
#       A shadow of GAME (keep SHADOW short: "folder + 1 + stream path" must
#       stay under 256 characters) with the example of examples/native-class:
#       the three .asi files, two objects and the script in gf2/cuba/local,
#       one more object in the zone $ZONE, the live module and the crash
#       module. Next to SHADOW it makes ws (the workspace), out (the built
#       files), exp and guids.tsv (the objects of the mod classes).
#   research/bench_platform.sh session SHADOW OUT
#       One session of the game in Havana (start … stop, to be run under the
#       lock of the bench): the objects, the messages, the Lua functions, the
#       filter, the hook, unloading and loading the zone, an autosave.
#   research/bench_platform.sh resume SHADOW OUT
#       A normal start with Return pressed through the menus until a level
#       runs ("Resume Game" loads the last save), then the same checks.
#
# Environment: DOCTOR_TOOLKIT (default target/debug/doctor-toolkit of this
# repository), BENCH_PREFIX (default …/wine-gf2-bench-s80), PORT (47880),
# ZONE (a zone stream of Havana), CMD_TXT (launch options of `setup`,
# default "-stream gf2 cuba"; empty for a normal start).
set -uo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
t=${DOCTOR_TOOLKIT:-$root/target/debug/doctor-toolkit}
bench=$root/tools/game-bench.sh
helper=$root/research/bench_platform.py
port=${PORT:-47880}
ZONE=${ZONE:-gf2/cuba/local/yuri/high/filler_056_01}
export DOCTOR_TOOLKIT=$t
export DOCTOR_TOOLKIT_CLASSES=$root/examples/native-class/classes.toml
export SERVER=xvfb DXVK=1 WINEDLLOVERRIDES=dinput8=n,b
export BENCH_PREFIX=${BENCH_PREFIX:-$HOME/.local/share/doctor_toolkit/wine-gf2-bench-s80}
unset DISPLAY

live() {
    local command=$1
    shift
    "$t" live "$command" "$@" --game "$shadow" 2>&1
}
say() { echo "=== $*"; }

install_link() {
    local options=${CMD_TXT--stream gf2 cuba}
    if [[ -n $options ]]; then
        "$t" live install "$1" --writes --port "$port" "--cmd-txt=$options" | tail -1
    else
        "$t" live install "$1" --writes --port "$port" | tail -1
    fi
}

setup() {
    local game=$1 shadow=$2 work
    work=$(dirname "$shadow")
    rm -rf "$shadow" "$work/ws" "$work/out" "$work/exp"
    "$bench" shadow "$game" "$shadow" gf2/cuba/local.str "$ZONE.str" | tail -1
    # Copies of the game folder's own modules keep the bench from starting.
    rm -f "$shadow/d3d9.dll" "$shadow"/scripts/doctor_live.* "$shadow/scripts/GF2ASI.asi"
    EXAMPLE_ZONE=$ZONE "$root/examples/native-class/build.sh" "$work/out" "$shadow" "$work/ws" 2>&1 |
        grep -E '^(built|data|error)'
    "$t" workspace apply "$shadow" --dir "$work/ws" | grep wrote
    cp "$work"/out/scripts/*.asi "$shadow/scripts/"
    install_link "$shadow"
    "$t" crash-report install "$shadow" | head -1
    "$t" logic-export "$shadow" -o "$work/exp" --type simgroup --stream gf2/cuba/local >/dev/null
    python3 "$helper" guids "$work/exp" >"$work/guids.tsv"
    cat "$work/guids.tsv"
}

# Waits until a level runs (the link sees the player); fails after $1 seconds.
wait_level() {
    local deadline=$((SECONDS + $1))
    while ((SECONDS < deadline)); do
        if live status --timeout 5 | grep -q 'player: present'; then
            return 0
        fi
        sleep 3
    done
    return 1
}

objects() {
    live objects --all --json --timeout 60 >"$out/world-$1.json"
    python3 "$helper" world "$out/world-$1.json" "$(dirname "$shadow")/guids.tsv"
}

info() { live lua 'return DoctorCounterInfo()' --yes; }

checks() {
    say "platform log"
    cat "$shadow/scripts/doctor_platform.log"
    say "status"
    live status
    say "objects"
    objects "$1"
    say "the Lua functions, called from the console of the link"
    live lua 'return DoctorCAdd(40, 2), DoctorCUpper("mod"), DoctorCounterInfo()' --yes
    say "the counter: two messages make it fire, the echo answers, the listener hears"
    live send doctor_example_count --yes
    info
    live send doctor_example_count --yes
    info
    say "the filter: doctor_example_blocked is swallowed (it would reset the counter)"
    live send doctor_example_count --yes
    live send doctor_example_blocked --yes
    live send doctor_example_count --yes
    info
    say "the hook of EclipseLog"
    live lua 'EclipseLog("a") EclipseLog("b") return DoctorCounterInfo()' --yes
    say "the rest of the functions of the platform, tried by the mod"
    live lua 'return DoctorSelfTest()' --yes
    live lua 'return DoctorGuid(DoctorObject()), DoctorGuid(1), DoctorSend("doctor_example_count", 2, true)' --yes
    info
    sleep 4
    say "… and 4 s later: the delayed message has come"
    info
    say "the script of the level: its start line, then the query"
    live send doctor_example_query --yes
    sleep 2
    live log --filter doctor_example
    say "errors of Lua"
    live log --errors | tail -5
}

finish() {
    "$bench" stop "$out"
    say "exceptions in wine.log"
    grep -c -i 'unhandled\|page fault' "$out/wine.log"
    say "crash reports"
    ls "$shadow/_doctor_toolkit/crashes" 2>/dev/null
}

session() {
    shadow=$1 out=$2
    mkdir -p "$out"
    rm -f "$shadow/scripts/doctor_platform.log"
    "$bench" start "$shadow" "$out" || return 1
    if wait_level 150; then
        sleep 8
        checks start
        for round in 1 2; do
            say "round $round: the zone is loaded: its DoctorCounter is created"
            live stream load "$ZONE" --wait --yes --timeout 90
            sleep 2
            info
            objects "loaded-$round"
            live send doctor_example_count --yes
            info
            say "round $round: the zone is unloaded: its DoctorCounter is destroyed"
            live stream unload "$ZONE" --wait --yes --timeout 90
            sleep 2
            info
            objects "unloaded-$round"
            live send doctor_example_count --yes
            info
        done
        say "an autosave"
        live send cp_do_autosave_now --yes
        sleep 8
        info
        say "a minute of the game with the objects"
        sleep 60
        live status
        info
        "$bench" do "$out" shot end
        say "platform log at the end"
        cat "$shadow/scripts/doctor_platform.log"
    else
        say "the level did not start"
        "$bench" do "$out" shot stuck
    fi
    finish
}

resume() {
    shadow=$1 out=$2
    mkdir -p "$out"
    rm -f "$shadow/scripts/doctor_platform.log"
    "$bench" start "$shadow" "$out" || return 1
    local n=0
    sleep 20
    until live status --timeout 5 | grep -q 'player: present'; do
        n=$((n + 1))
        if ((n > 40)); then
            break
        fi
        "$bench" do "$out" key Return
        sleep 6
        if ((n % 4 == 0)); then
            "$bench" do "$out" shot "menu-$n"
        fi
    done
    if wait_level 60; then
        sleep 12
        "$bench" do "$out" shot loaded
        checks resume
    else
        say "no level after the menus"
        live status
    fi
    finish
}

case ${1:-} in
setup) setup "$2" "$3" ;;
session) session "$2" "$3" ;;
resume) resume "$2" "$3" ;;
*)
    sed -n '2,25p' "$0"
    exit 2
    ;;
esac

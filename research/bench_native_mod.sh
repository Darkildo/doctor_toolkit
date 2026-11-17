#!/usr/bin/env bash
# Bench check of native mods in the mod manager (stage 81,
# docs/analysis/game-bench.md §45): the example of examples/native-class as
# one mod of format version 5, installed by the mod manager.
#
#   research/bench_native_mod.sh setup GAME SHADOW
#       A shadow of GAME (keep SHADOW short: "folder + 1 + stream path" must
#       stay under 256 characters) without the player's own modules and,
#       unless KEEP_LOADER=1, without his ASI loader (dinput8.dll): the mod
#       manager must install the toolkit's. Next to SHADOW: out (the built
#       modules), ws (the workspace of the data), mod.zip (the example,
#       `--native-required`) and soft.zip (the same without that flag).
#   research/bench_native_mod.sh install SHADOW [OLD_TOOLKIT]
#       What the mod manager does with the two mods, step by step, with the
#       files of scripts/ after each: no answer, "data only", "allow",
#       apply. OLD_TOOLKIT (a doctor-toolkit of an earlier version) is asked
#       to read mod.zip: it must refuse. Ends with the mod of the example
#       written into SHADOW, the live module and the crash module.
#   research/bench_native_mod.sh session SHADOW OUT
#       One session of the game in Havana (start … stop, to be run under the
#       lock of the bench): the objects of the mod's classes, its messages
#       and Lua functions, and a walk (the keyboard through the loader).
#   research/bench_native_mod.sh remove GAME SHADOW
#       Switching the mod off, applying again, "Restore originals",
#       uninstalling: the files of scripts/ and of the mod manager after
#       each; then `game-bench.sh verify`.
#
# Environment: DOCTOR_TOOLKIT (default target/debug/doctor-toolkit of this
# repository), BENCH_PREFIX (default …/wine-gf2-bench-s81), PORT (47881).
set -uo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
t=${DOCTOR_TOOLKIT:-$root/target/debug/doctor-toolkit}
bench=$root/tools/game-bench.sh
helper=$root/research/bench_platform.py
port=${PORT:-47881}
id=doctor-native-class
export DOCTOR_TOOLKIT=$t
export DOCTOR_TOOLKIT_CLASSES=$root/examples/native-class/classes.toml
export SERVER=xvfb DXVK=1 WINEDLLOVERRIDES=dinput8=n,b
export BENCH_PREFIX=${BENCH_PREFIX:-$HOME/.local/share/doctor_toolkit/wine-gf2-bench-s81}
unset DISPLAY

say() { echo "=== $*"; }
# Runs a command of the toolkit and shows its exit code when it fails.
run() {
    echo "\$ doctor-toolkit $*" | sed "s|$shadow|SHADOW|g; s|$work|WORK|g"
    "$t" "$@" 2>&1 | sed "s|$shadow|SHADOW|g; s|$work|WORK|g"
    local code=${PIPESTATUS[0]}
    ((code == 0)) || echo "(exit code $code)"
}
# The executable files of the shadow and what the mod manager keeps.
files() {
    echo "--- game folder: $(cd "$shadow" && ls dinput8.dll 2>/dev/null || echo 'no dinput8.dll')"
    echo "--- scripts/: $(ls "$shadow/scripts" 2>/dev/null | tr '\n' ' ')"
    if [[ -f $shadow/_doctor_toolkit/mods/native.json ]]; then
        echo "--- _doctor_toolkit/mods/native.json:"
        sed 's/^/    /' "$shadow/_doctor_toolkit/mods/native.json"
    else
        echo "--- no _doctor_toolkit/mods/native.json"
    fi
}
live() {
    local command=$1
    shift
    "$t" live "$command" "$@" --game "$shadow" 2>&1
}

setup() {
    local game=$1
    rm -rf "$shadow" "$work/ws" "$work/out" "$work/exp" "$work/mod.zip" "$work/soft.zip"
    "$bench" shadow "$game" "$shadow" gf2/cuba/local.str | tail -1
    # Copies of the game folder's own modules keep the bench from starting.
    rm -f "$shadow/d3d9.dll" "$shadow"/scripts/doctor_live.* "$shadow"/scripts/GF2ASI.*
    if [[ -z ${KEEP_LOADER:-} ]]; then
        rm -f "$shadow/dinput8.dll"
    fi
    "$root/examples/native-class/build.sh" "$work/out" "$shadow" "$work/ws" "$work/mod.zip" 2>&1 |
        grep -E '^(built|data|the mod|error|warning)'
    # The same mod, saying it can do without its code.
    run mods build "$shadow" -o "$work/soft.zip" --name "Doctor native class (soft)" \
        --id doctor-native-soft --mod-version 1.0 \
        --native "$work/out/scripts/doctor_example.asi" \
        --native "$work/out/scripts/doctor_example_c.asi" --dir "$work/ws"
    files
}

install() {
    local old=${1:-}
    say "the mod file"
    run mods info "$work/mod.zip"
    if [[ -n $old ]]; then
        say "an older program ($("$old" --version)) reads the mod"
        echo "\$ OLD mods info WORK/mod.zip"
        "$old" mods info "$work/mod.zip" 2>&1 | sed "s|$work|WORK|g"
        echo "(exit code ${PIPESTATUS[0]})"
        echo "\$ OLD mods install SHADOW WORK/mod.zip"
        "$old" mods install "$shadow" "$work/mod.zip" 2>&1 | sed "s|$shadow|SHADOW|g; s|$work|WORK|g"
        echo "(exit code ${PIPESTATUS[0]})"
        ls "$shadow/_doctor_toolkit/mods" 2>/dev/null
    fi

    say "a mod that can do without its code: no answer"
    run mods install "$shadow" "$work/soft.zip"
    run mods check "$shadow"
    run mods apply "$shadow"
    files
    say "… 'data only'"
    run mods native "$shadow" doctor-native-soft --data-only
    run mods apply "$shadow"
    run mods native "$shadow"
    files
    say "… taken away"
    run mods unapply "$shadow"
    run mods uninstall "$shadow" doctor-native-soft
    files

    say "the mod of the example (its code is required): no answer"
    run mods install "$shadow" "$work/mod.zip"
    run mods apply "$shadow"
    files
    say "… 'data only' is not an answer for it"
    run mods native "$shadow" "$id" --data-only
    say "… allowed"
    run mods native "$shadow" "$id" --allow
    files
    run mods apply "$shadow"
    run mods native "$shadow"
    run mods status "$shadow"
    files
    say "the live module and the crash module, to look into the game"
    "$t" live install "$shadow" --writes --port "$port" "--cmd-txt=-stream gf2 cuba" 2>&1 | tail -1
    "$t" crash-report install "$shadow" 2>&1 | sed -n 1p
    rm -rf "$work/exp"
    "$t" logic-export "$shadow" -o "$work/exp" --type simgroup --stream gf2/cuba/local >/dev/null
    python3 "$helper" guids "$work/exp" >"$work/guids.tsv"
    cat "$work/guids.tsv"
    files
}

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

info() { live lua 'return DoctorCounterInfo()' --yes; }

session() {
    local out=$1
    mkdir -p "$out"
    rm -f "$shadow/scripts/doctor_platform.log"
    "$bench" start "$shadow" "$out" || return 1
    if wait_level 150; then
        sleep 8
        say "platform log"
        cat "$shadow/scripts/doctor_platform.log"
        say "status"
        live status
        say "objects of the classes of the mod"
        live objects --all --json --timeout 60 >"$out/world.json"
        python3 "$helper" world "$out/world.json" "$work/guids.tsv"
        say "the Lua functions of the mod"
        live lua 'return DoctorCAdd(40, 2), DoctorCUpper("mod"), DoctorCounterInfo()' --yes
        say "the object answers: two messages make the counter fire, the echo answers"
        live send doctor_example_count --yes
        info
        live send doctor_example_count --yes
        info
        say "the script of the level"
        live send doctor_example_query --yes
        sleep 2
        live log --filter doctor_example
        say "errors of Lua"
        live log --errors | tail -5
        say "the rest of the platform, tried by the mod"
        live lua 'return DoctorSelfTest()' --yes
        say "input (DirectInput8Create goes through the loader): the player before and after holding W"
        live player
        "$bench" do "$out" hold w 3
        live player
        "$bench" do "$out" shot end
    else
        say "the level did not start"
        "$bench" do "$out" shot stuck
        cat "$shadow/scripts/doctor_platform.log" 2>/dev/null
    fi
    "$bench" stop "$out"
    say "exceptions in wine.log"
    grep -c -i 'unhandled\|page fault' "$out/wine.log"
    say "crash reports"
    ls "$shadow/_doctor_toolkit/crashes" 2>/dev/null
}

remove() {
    local game=$1
    say "the mod is switched off: its code leaves at once (the loader stays for the other modules)"
    run mods disable "$shadow" "$id"
    files
    say "switched on and applied: back"
    run mods enable "$shadow" "$id"
    run mods apply "$shadow"
    files
    say "the modules of the bench are taken away"
    "$t" live uninstall "$shadow" 2>&1 | tail -1
    "$t" crash-report uninstall "$shadow" 2>&1 | tail -1
    rm -f "$shadow"/scripts/doctor_live.log "$shadow"/scripts/doctor_platform.log
    files
    say "restore originals"
    run mods unapply "$shadow"
    files
    run mods native "$shadow"
    say "uninstalled"
    run mods uninstall "$shadow" --all
    files
    echo "--- _doctor_toolkit: $(cd "$shadow/_doctor_toolkit" 2>/dev/null && find . -type f | sort | tr '\n' ' ')"
    say "verify"
    "$bench" verify "$game" "$shadow"
}

case ${1:-} in
setup)
    shadow=$3 work=$(dirname "$3")
    setup "$2"
    ;;
install)
    shadow=$2 work=$(dirname "$2")
    install "${3:-}"
    ;;
session)
    shadow=$2 work=$(dirname "$2")
    session "$3"
    ;;
remove)
    shadow=$3 work=$(dirname "$3")
    remove "$2"
    ;;
*)
    sed -n '2,29p' "$0"
    exit 2
    ;;
esac

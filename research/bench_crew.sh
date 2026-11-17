#!/usr/bin/env bash
# One session of the game on the bench for the crew experiments of stage 82
# (docs/analysis/game-bench.md §44).
#
#   research/bench_crew.sh SHADOW OUT STEPS.sh
#
# SHADOW is a shadow copy with the live module installed (`doctor-toolkit live
# install SHADOW --writes --port N "--cmd-txt=-stream gf2 queens"`); keep it in
# a short folder (the game builds stream paths in 256 bytes). STEPS.sh is
# sourced while the game runs; it may use:
#   bench STEP [ARG...]   a step of tools/game-bench.sh (shot, key, click, wait, until …)
#   live ARGS...          `doctor-toolkit live ARGS --game SHADOW`
#   wait_level [SECONDS]  wait until the module answers and the player stands in a level
# The whole session holds the lock of the shared bench (BENCH_LOCK).
set -euo pipefail
shadow=${1:?shadow}
out=${2:?output directory}
steps=${3:?steps script}
here=$(cd "$(dirname "$0")/.." && pwd)
t=${DOCTOR_TOOLKIT:-doctor-toolkit}
lock=${BENCH_LOCK:?BENCH_LOCK: the lock file of the shared bench}
export SERVER=xvfb DXVK=${DXVK:-1} WINEDLLOVERRIDES=${WINEDLLOVERRIDES:-dinput8=n,b}
export BENCH_PREFIX=${BENCH_PREFIX:-$HOME/.local/share/doctor_toolkit/wine-gf2-bench-s82}
unset DISPLAY

bench() { "$here/tools/game-bench.sh" do "$out" "$@"; }
live() { "$t" live "$@" --game "$shadow"; }
wait_level() {
    local limit=${1:-150} i
    for ((i = 0; i < limit; i += 3)); do
        if live player 2>/dev/null | grep -q .; then return 0; fi
        sleep 3
    done
    echo "bench_crew: the player did not appear in $limit s" >&2
    return 1
}

mkdir -p "$out"
exec 9>"$lock"
flock 9
"$here/tools/game-bench.sh" start "$shadow" "$out"
status=0
( set +e; source "$steps" ) || status=$?
"$here/tools/game-bench.sh" stop "$out" || true
flock -u 9
exit "$status"

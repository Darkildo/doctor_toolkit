#!/usr/bin/env bash
# Runs sessions of research/bench_draws.sh one after another and stops at the
# first sign of trouble with the graphics card (a new line of the kernel log
# about it, or nvidia-smi failing) — stage 101, game-bench.md §46.
#
#   research/bench_draws_batch.sh SHADOW OUT_ROOT STEPS.sh CFG NAME:CITY...
set -uo pipefail
shadow=$1 root=$2 steps=$3 cfg=$4
shift 4
here=$(cd "$(dirname "$0")" && pwd)
for item in "$@"; do
    name=${item%%:*} city=${item##*:}
    echo "== $name ($city) $(date +%T)"
    "$here/bench_draws.sh" "$shadow" "$root/$name" "$city" "$cfg" "$steps" >"$root/$name.log" 2>&1
    echo "   exit $? at $(date +%T); crash reports: $(find "$root/$name/crash" -type f ! -name doctor_crash.log | wc -l); exceptions in wine.log: $(grep -ciE 'unhandled|exception|page fault' "$root/$name/wine.log")"
    if [[ -s $root/$name/kernel.txt ]] || ! nvidia-smi >/dev/null 2>&1; then
        echo "GRAPHICS CARD TROUBLE after $name:"
        cat "$root/$name/kernel.txt"
        exit 3
    fi
done

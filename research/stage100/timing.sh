#!/usr/bin/env bash
# The timing series of stage 100; run it in one hold of the bench lock:
#
#   flock $BENCH_LOCK research/stage100/timing.sh WORKSPACE [REPEATS] [CITY]
#
# REPEATS times (default 3), in turn:
#   none     the game as it is (the module installed, nothing staged)
#   rebuilt  WORKSPACE through rebuilt streams (stage 46)
#   memory   WORKSPACE through resource patches (stage 100)
# Each session: timing.body.sh. Before each the staging itself is timed
# (`live install`): seconds and the bytes of real files in the staged copy.
# Results: $S/out/t-MODE-N/timing.txt; timing_summary.py prints the table.
set -uo pipefail
here=$(dirname "$0")
. "$here/env.sh"
ws=$1
repeats=${2:-3}
city=${3:-cuba}
for n in $(seq 1 "$repeats"); do
    for mode in none rebuilt memory; do
        args=()
        [[ $mode != none ]] && args+=(--apply --workspace-dir "$ws")
        [[ $mode == memory ]] && args+=(--resources-in-memory)
        out=$S/out/t-$mode-$n
        stage_line="0 0"
        if [[ $mode != none ]]; then
            t0=$(date +%s.%N)
            "$T" live install "$SHADOWS/shadow" --port "$PORT" "${args[@]}" >/dev/null 2>&1
            t1=$(date +%s.%N)
            staged=$(ls -d "$S"/cache/overlay-* | head -1)
            bytes=$(find "$staged" -type f -printf '%s\n' | paste -sd+ | bc)
            "$T" live uninstall "$SHADOWS/shadow" >/dev/null 2>&1
            stage_line="$(echo "$t1 $t0" | awk '{ printf "%.2f", $1 - $2 }') $bytes"
        fi
        bash "$here/session.sh" "$out" "$SHADOWS/shadow" "$city" "$here/timing.body.sh" "${args[@]}"
        echo "stage $stage_line" >>"$out/timing.txt"
    done
done

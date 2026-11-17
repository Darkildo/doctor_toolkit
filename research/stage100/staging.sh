#!/usr/bin/env bash
# staging.sh WORKSPACE [REPEATS]: how long the preparation of a launch takes
# and how much it writes — `live install --apply` on $SHADOWS/shadow with
# rebuilt streams (stage 46) and with resource patches (stage 100). No game
# is started. Prints seconds (wall), peak memory of the toolkit and the bytes
# of real files in the staged copy for every repeat.
set -uo pipefail
here=$(dirname "$0")
. "$here/env.sh"
ws=$1
repeats=${2:-3}
export LC_ALL=C
for mode in rebuilt memory; do
    args=(--apply --workspace-dir "$ws")
    [[ $mode == memory ]] && args+=(--resources-in-memory)
    for n in $(seq 1 "$repeats"); do
        line=$({ /usr/bin/time -f "%e s, %M KiB peak" "$T" live install "$SHADOWS/shadow" \
            --port "$PORT" "${args[@]}" >/dev/null; } 2>&1 | tail -1)
        staged=$(ls -d "$S"/cache/overlay-* | head -1)
        bytes=$(find "$staged" -type f -printf '%s\n' | paste -sd+ | bc)
        files=$(find "$staged" -type f | wc -l)
        echo "$mode $n: $line, $bytes bytes in $files real file(s), $(grep -c '^redirect=' "$SHADOWS/shadow/scripts/doctor_live.cfg") redirect(s), $(grep -c '^resources=' "$SHADOWS/shadow/scripts/doctor_live.cfg") patch(es)"
        "$T" live uninstall "$SHADOWS/shadow" >/dev/null 2>&1
    done
done

#!/usr/bin/env bash
# run.sh NAME [INSTALL_ARG...]: one session of the game with the project
# staged on top of the base mod written into the shadow; the state of the
# shadow before and after.
set -uo pipefail
here=$(dirname "$(realpath "$0")")
. "$here/env.sh"
name=$1
shift
bash "$here/state.sh" "$S/${SHADOW_NAME:-shadow}" >"$S/state-before-$name.txt"
sh=${SHADOW_NAME:-shadow}
if [[ ${PLAIN:-0} == 1 ]]; then
    args=()
else
    args=(--apply --workspace-dir "$S/proj/workspace")
fi
flock "$BENCH_LOCK" bash "$ROOT/research/stage100/session.sh" "$S/out-$name" "$S/$sh" cuba \
    "$here/body.sh" "${args[@]}" "$@"
echo "session exit: $?"
bash "$here/state.sh" "$S/${SHADOW_NAME:-shadow}" >"$S/state-after-$name.txt"
if cmp -s "$S/state-before-$name.txt" "$S/state-after-$name.txt"; then
    echo "state of the shadow: the same ($(wc -l <"$S/state-after-$name.txt") entries)"
else
    echo "state of the shadow: DIFFERS"
    diff "$S/state-before-$name.txt" "$S/state-after-$name.txt" | head -20
fi

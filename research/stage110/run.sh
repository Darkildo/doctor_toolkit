#!/usr/bin/env bash
# run.sh NAME SHADOW [INSTALL_ARG...]: one session of the game on the shadow
# $S/SHADOW; with `--mods` the enabled mods are staged (folded by the mod
# manager of the staged copy), without it the shadow is played as it is.
set -uo pipefail
here=$(dirname "$(realpath "$0")")
. "$here/env.sh"
name=$1
sh=$2
shift 2
bash "$ROOT/research/stage107/state.sh" "$S/$sh" >"$S/state-before-$name.txt"
flock "$BENCH_LOCK" bash "$ROOT/research/stage100/session.sh" "$S/out-$name" "$S/$sh" cuba \
    "$here/body.sh" "$@"
echo "session exit: $?"
bash "$ROOT/research/stage107/state.sh" "$S/$sh" >"$S/state-after-$name.txt"
if cmp -s "$S/state-before-$name.txt" "$S/state-after-$name.txt"; then
    echo "state of the shadow: the same ($(wc -l <"$S/state-after-$name.txt") entries)"
else
    echo "state of the shadow: DIFFERS"
    diff "$S/state-before-$name.txt" "$S/state-after-$name.txt" | head -20
fi

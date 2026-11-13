#!/usr/bin/env bash
# A "Command of your own" for `doctor-toolkit play` that starts the game in
# the bench (hidden nested display, bench prefix) instead of the user's Wine:
#
#   doctor-toolkit play SHADOW --method command \
#       --command 'tools/bench/play-command.sh {dir}' --apply --revert-after
#
# {dir} must be a shadow directory (tools/game-bench.sh shadow). The game
# runs for DURATION seconds (default 70) with screenshots at SHOTS (default
# "50 65") and is stopped; the toolkit then sees it exit. Output goes to
# PLAY_OUT (default {dir}/../play-out). Other bench variables (BENCH_PREFIX,
# REFS, ...) pass through.
set -euo pipefail
(($# == 1)) || {
    echo "usage: play-command.sh SHADOW" >&2
    exit 2
}
here=$(dirname "$(realpath "$0")")
shadow=$(realpath "$1")
out=${PLAY_OUT:-$(dirname "$shadow")/play-out}
DURATION=${DURATION:-70} SHOTS=${SHOTS:-50 65} exec "$here/../game-bench.sh" run "$shadow" "$out"

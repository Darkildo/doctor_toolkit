#!/usr/bin/env bash
# try.sh NAME WORKSPACE [BUILD_ARG...]: one mod built from WORKSPACE (with
# the build arguments: --no-deltas, keys of the changes) alone in the shadow
# `only-a`, staged for a session of the game. For finding what in a mod the
# game does not take.
set -uo pipefail
here=$(dirname "$(realpath "$0")")
. "$here/env.sh"
name=$1
ws=$2
shift 2
"$T" mods uninstall "$S/only-a" --all >/dev/null 2>&1
"$T" mods build "$S/shadow" --name "Try $name" --id "try-$name" --dir "$ws" -o "$S/try-$name.zip" "$@" |
    grep -E "^built|stored as deltas"
"$T" mods install "$S/only-a" "$S/try-$name.zip" | head -1
bash "$here/run.sh" "try-$name" only-a --mods 2>&1 |
    grep -E "crashed|page fault|doctor|== |matched|session exit|state of|exception lines"

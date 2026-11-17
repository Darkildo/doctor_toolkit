#!/usr/bin/env bash
# mkshadow.sh NAME [REL_PATH...]: a fresh shadow copy of the game in $SHADOWS/NAME
# without the files of the player's own setup that stop the bench (the
# player's DXVK d3d9.dll, the live module and GF2ASI), with the crash module.
set -euo pipefail
. "$(dirname "$0")/env.sh"
name=$1
shift
"$ROOT/tools/game-bench.sh" shadow "$GAME" "$SHADOWS/$name" "$@" | tail -1
rm -f "$SHADOWS/$name/d3d9.dll" "$SHADOWS/$name"/scripts/doctor_live.* \
    "$SHADOWS/$name/scripts/GF2ASI.asi" "$SHADOWS/$name/scripts/GF2ASI.ini"
if [[ ${CRASH:-1} == 1 ]]; then
    "$T" crash-report install "$SHADOWS/$name" | tail -1
fi
echo "$SHADOWS/$name"

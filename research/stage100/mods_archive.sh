#!/usr/bin/env bash
# mods_archive.sh: the files outside the streams of the mods of
# $SHADOWS/shadow-mods staged with resource patches — the overlay archive
# and the shifted archives of the game — against `mods apply`
# ($S/expected-mods). No game is started.
set -uo pipefail
here=$(dirname "$0")
. "$here/env.sh"
sh=$SHADOWS/shadow-mods
"$T" live install "$sh" --port 47955 --mods --resources-in-memory 2>&1 | grep -v '^warning' | head -4
for d in "$S"/cache/overlay-*; do
    # Only the staged copy with an overlay archive of its own.
    [[ -f $d/game/GODFATH0.viv && ! -L $d/game/GODFATH0.viv ]] || continue
    for f in "$d"/game/*.viv; do
        if [[ -L $f ]]; then
            echo "$(basename "$f") -> $(readlink "$f")"
        else
            echo "$(basename "$f"): a file of $(stat -c %s "$f") bytes"
        fi
    done
    cmp "$d/game/GODFATH0.viv" "$S/expected-mods/GODFATH0.viv" && echo "the overlay archive is the file apply writes"
done
echo "resources= lines: $(grep -c '^resources=' "$sh/scripts/doctor_live.cfg"), redirect= lines: $(grep -c '^redirect=' "$sh/scripts/doctor_live.cfg")"
grep -i 'viv' "$sh/scripts/doctor_live.cfg" | cut -c1-90
"$T" live uninstall "$sh" | tail -1

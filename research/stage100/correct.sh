#!/usr/bin/env bash
# correct.sh CITY MODE [NAME]: one correctness session in CITY (cuba, miami,
# queens). Takes the bench lock for the session of the game only.
#   MODE memory   the test workspace $S/ws (mkws.py) through resource
#                 patches (stage 100); the module writes every composed
#                 stream next to its patch and they are compared with the
#                 streams of $S/expected (a shadow with `workspace apply`)
#   MODE rebuilt  the same workspace through rebuilt streams (stage 46)
#   MODE mods     the enabled mods of the mod manager of $S/shadow-mods
#                 through resource patches; compared with $S/expected-mods
#                 (a shadow with the same mods and `mods apply`)
set -uo pipefail
here=$(dirname "$0")
. "$here/env.sh"
city=$1
mode=$2
name=${3:-$mode-$city}
out=$S/out/$name
shadow=$SHADOWS/shadow
export EXPECTED=$S/expected
case $mode in
memory) args=(--apply --workspace-dir "$S/ws" --resources-in-memory --resources-dump) ;;
rebuilt) args=(--apply --workspace-dir "$S/ws") ;;
mods)
    args=(--mods --resources-in-memory --resources-dump)
    shadow=$SHADOWS/shadow-mods
    export EXPECTED=$S/expected-mods
    ;;
*)
    echo "unknown mode $mode" >&2
    exit 1
    ;;
esac
if [[ $mode == mods ]]; then
    # Streams the three test mods change that are loaded at the spawn point
    # of Havana, and the new zone of the yard mod (a cell of the world grid
    # 28 m from the spawn point).
    export STREAMS=${STREAMS:-"gf2/cuba gf2/globals_global gf2/globalfolder/characters/allcharacters gf2/cuba/local/yuri/high/terrain1 gf2/cuba/local/yuri/high/terrain2 gf2/cuba/local/yuri/high/terrain3 gf2/cuba/local/yuri/high/terrain4 gf2/testmods/havana_yard"}
    export NEW=""
else
    extra=""
    [[ $city == cuba ]] && extra=" gf2/cuba/local/yuri/high/terrain3"
    export STREAMS="gf2/$city/local gf2/${city}_global gf2/globals_global$extra"
fi
KEEP_STAGED=1 flock "$BENCH_LOCK" bash "$here/session.sh" "$out" "$shadow" "$city" \
    "$here/check.body.sh" "${args[@]}"
status=$?
if [[ $mode != rebuilt ]]; then
    staged=$(ls -d "$S"/cache/overlay-*/game | head -1)
    {
        find "$staged" -name '*.composed' | sort | while read -r dump; do
            rel=${dump#"$staged"/}
            rel=${rel%.composed}
            bash "$here/compare_streams.sh" "$dump" "$EXPECTED/$rel"
        done
        # Everything else the game gets from the staged copy is a file
        # `apply` writes as it is: tables of contents, the new streams, the
        # overlay archive.
        grep '^redirect=' "$out/doctor_live.cfg" | sed 's/^redirect=//; s/|.*//' |
            while read -r rel; do
                [[ $rel == cmd.txt ]] && continue
                # The keys are lower case; the files have the game's spelling.
                target=$(find "$EXPECTED" -maxdepth 6 -ipath "$EXPECTED/$rel" | head -1)
                mine=$(find "$staged" -maxdepth 6 -ipath "$staged/$rel" | head -1)
                if [[ -L $mine ]]; then
                    echo "another file of the game: $rel -> $(readlink "$mine")"
                elif [[ -n $target && -n $mine ]] && cmp -s "$mine" "$target"; then
                    echo "same file: $rel"
                else
                    echo "DIFFERENT file: $rel"
                fi
            done
    } >"$out/composed.txt" 2>&1
    cat "$out/composed.txt"
fi
"$T" live uninstall "$shadow" | tail -1
exit $status

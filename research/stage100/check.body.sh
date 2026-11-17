# Body of a correctness session (sourced by session.sh): the streams the
# module composed, the resources in the game's memory against the streams
# `apply` wrote into $EXPECTED, the new stream loaded on request.
#   EXPECTED  folder with the workspace applied (the result of `apply`)
#   STREAMS   changed streams to check (default: those of mkws.py for CITY)
#   NEW       the new stream to load (default gf2/doctor/s100CITY; "" for none)
EXPECTED=${EXPECTED:-$S/expected}
STREAMS=${STREAMS:-"gf2/$CITY/local gf2/${CITY}_global gf2/globals_global"}
NEW=${NEW-gf2/doctor/s100$CITY}
L status | tee -a "$OUT/run.log"
L player | tee -a "$OUT/run.log"
shot spawn
L files >"$OUT/files.txt" 2>&1
L resources | tee "$OUT/resources.txt"
L resources --json >"$OUT/resources.json" 2>&1
if [ -n "$NEW" ]; then
    log "loading $NEW"
    L stream load "$NEW" --wait --yes 2>&1 | tee -a "$OUT/run.log"
    frames 30
    STREAMS="$STREAMS $NEW"
fi
log "resources in memory against $EXPECTED"
# Changed and added resources in full, the others by their first 4 KiB.
# shellcheck disable=SC2086
L resource-check "$EXPECTED" $STREAMS --original "$GAME" >"$OUT/check.txt" 2>&1
grep -E "^(gf2|  changed|  added|in all)" "$OUT/check.txt" | tee -a "$OUT/run.log"
# shellcheck disable=SC2086
L resource-check "$EXPECTED" $STREAMS --original "$GAME" --json >"$OUT/check.json" 2>&1
L profile --memory >"$OUT/profile.txt" 2>&1
L streams >"$OUT/streams1.txt" 2>&1
frames 120
shot end
L status | tee -a "$OUT/run.log"
L resources >"$OUT/resources-end.txt" 2>&1

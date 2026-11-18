# Body of the stage 107 session (sourced by research/stage100/session.sh):
# the files the game got, the resources of the zone in its memory against
# the shadow with the base mod and the project written by the mod manager.
STREAM=gf2/cuba/local/yuri/high/terrain3
L status | tee -a "$OUT/run.log"
L player | tee -a "$OUT/run.log"
shot spawn
L files >"$OUT/files.txt" 2>&1
cat "$OUT/files.txt" | tee -a "$OUT/run.log"
L resources >"$OUT/resources.txt" 2>&1
log "resources in memory against $S/expected"
L resource-check "$S/expected" "$STREAM" gf2/globals_global --original "$GAME" >"$OUT/check.txt" 2>&1
grep -E "^(gf2|  changed|  added|in all)" "$OUT/check.txt" | tee -a "$OUT/run.log"
L resource-check "$S/expected" "$STREAM" gf2/globals_global --original "$GAME" --json >"$OUT/check.json" 2>&1
D hold w 5
frames 30
shot walk1
D hold w 4
frames 30
shot end
L status | tee -a "$OUT/run.log"

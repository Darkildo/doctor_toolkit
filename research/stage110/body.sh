# Body of the stage 110 session (sourced by research/stage100/session.sh):
# the new zones among the loaded streams and the new objects in the game.
L status | tee -a "$OUT/run.log"
L player | tee -a "$OUT/run.log"
frames 60
shot spawn
L files >"$OUT/files.txt" 2>&1
cat "$OUT/files.txt" | tee -a "$OUT/run.log"
log "new zones among the loaded streams:"
L streams >"$OUT/streams.txt" 2>&1
grep -i "doctor" "$OUT/streams.txt" | tee -a "$OUT/run.log"
log "objects of the mods in the game:"
while read -r who guid; do
    echo "== $who $guid" | tee -a "$OUT/run.log"
    L find "$guid" 2>&1 | head -4 | tee -a "$OUT/run.log"
done <"$S/guids.txt"
L objects --radius 25 >"$OUT/objects.txt" 2>&1
log "objects within 25 m: $(grep -c . "$OUT/objects.txt")"
D hold s 2
frames 30
shot back
L status | tee -a "$OUT/run.log"

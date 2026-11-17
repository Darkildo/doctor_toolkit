export LC_NUMERIC=C
# Steps of research/bench_draws.sh: merging on while the player is moved
# 150-300 m and back (zones unload and load, their index buffers are written
# anew, the module's copies and joined runs are dropped) — game-bench.md §46.
# Configuration: `draw_merge=1;draw_log=10`. HOPS is a list of "dx,dz" from
# the spawn point, WAIT the seconds after each (default 55).
wait_level 200 || exit 1
sleep 20
read -r x y z _ < <(live player)
echo "spawn at $x $y $z; streams: $(live streams | wc -l)"
bench shot s00
n=0
for hop in ${HOPS:-200,0 200,200 0,250 0,0 -150,0 150,-150 0,0}; do
    n=$((n + 1))
    to=$(awk -v x="$x" -v y="$y" -v z="$z" -v h="$hop" 'BEGIN {split(h, d, ","); printf "%.1f,%.1f,%.1f", x + d[1], y + 3, z + d[2]}')
    "$t" live teleport --yes --game "$shadow" -- "$to" || echo "teleport to $to failed"
    sleep "${WAIT:-55}"
    echo "hop $n to $to: player $(live player | head -1); streams: $(live streams | wc -l)"
    bench shot "$(printf 's%02d' "$n")"
done
live profile >"$out/profile.txt" 2>&1

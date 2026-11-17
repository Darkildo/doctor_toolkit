export LC_NUMERIC=C
# Steps of research/bench_draws.sh: the picture with merging off and on
# (docs/analysis/game-bench.md §38 and §46). The configuration needs
# `draw_merge=1;draw_trace=1` (the file doctor_render_draws.off is looked for
# only with draw_trace). The clock of the day stands, the game is stopped and
# stepped STEP frames (default 4) before every shot; 12 shots a view in the
# order off, off, on, on, …: p00_off.png … (the player's camera) and
# t00_off.png … (the free camera 25 m above him). Then
#   python3 research/frame_diff.py --states OUT/p*.png
#   python3 research/frame_diff.py --states OUT/t*.png
wait_level 200 || exit 1
sleep "${SETTLE:-45}"
live time 12:00 --rate 0 --yes
sleep 5
read -r x y z _ < <(live player)
echo "player at $x $y $z"
live pause
for view in p t; do
    if [[ $view == t ]]; then
        "$t" live freecam --pitch -20 --yes --game "$shadow" -- "$x,$(awk -v y="$y" 'BEGIN {print y + 25}'),$z"
    fi
    for i in 0 1 2 3 4 5 6 7 8 9 10 11; do
        if (((i / 2) % 2 == 0)); then state=off; merge_off; else state=on; merge_on; fi
        for ((k = 0; k < ${STEP:-4}; k++)); do live continue --step >/dev/null; done
        sleep 0.7
        bench shot "$(printf '%s%02d_%s' "$view" "$i" "$state")"
    done
done
live freecam --release --yes
merge_on
live continue
sleep 3
# One frame of calls with merging on: the counts under this Direct3D.
printf on >"$shadow/scripts/doctor_render_trace.go"
sleep 5
bench shot end

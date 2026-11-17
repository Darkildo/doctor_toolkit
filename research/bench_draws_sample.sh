export LC_NUMERIC=C
# Steps of research/bench_draws.sh: where the render thread stands and what a
# frame costs it, by `live profile` (game-bench.md §46). With
# `draw_merge=1;draw_trace=1` the states alternate (the file
# doctor_render_draws.off): ROUNDS (default 3) of "on, off"; with
# `draw_merge=0` (no hooks on the device at all) the same windows without a
# change of state. For every window of WINDOW seconds (default 30): the
# presents and the processor seconds of the render thread before and after
# (OUT/cpu.txt: state, presents, render thread ms a frame, main thread ms a
# frame) and a sample of 8 s (OUT/sample_<n>_<state>.txt).
wait_level 200 || exit 1
sleep "${SETTLE:-40}"
alternate=0
grep -q 'draw_trace=1' "$shadow/scripts/doctor_render.cfg" && alternate=1
numbers() {
    live profile | awk '
        /present\(s\)/ {for (i = 1; i <= NF; i++) if ($i ~ /^present\(s\)/) p = $(i - 1)}
        /^CPU:/ {m = $3; r = $10}
        END {print p, m, r}'
}
n=0
for ((round = 0; round < ${ROUNDS:-3}; round++)); do
    for state in on off; do
        n=$((n + 1))
        if ((alternate)); then
            if [[ $state == off ]]; then merge_off; else merge_on; fi
            sleep 2
        else
            state=none
        fi
        read -r p0 m0 r0 < <(numbers)
        sleep "${WINDOW:-30}"
        read -r p1 m1 r1 < <(numbers)
        awk -v s="$state" -v p=$((p1 - p0)) -v m0="$m0" -v m1="$m1" -v r0="$r0" -v r1="$r1" \
            'BEGIN {printf "%s %d presents, render thread %.3f ms a frame, main thread %.3f ms a frame\n", s, p, (r1 - r0) * 1000 / p, (m1 - m0) * 1000 / p}' |
            tee -a "$out/cpu.txt"
        live profile --sample 8000 >"$out/$(printf 'sample_%02d_%s' "$n" "$state").txt" 2>&1
    done
done
merge_on

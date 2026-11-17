#!/usr/bin/env bash
# One session of the game on the bench for the draw call experiments of
# stage 101 (docs/analysis/game-bench.md §46).
#
#   research/bench_draws.sh SHADOW OUT CITY CFG STEPS.sh
#
# SHADOW is a shadow copy with the live module installed (`doctor-toolkit live
# install SHADOW --writes --port N`) and, if wanted, the crash module; keep it
# in a short folder (the game builds stream paths in 256 bytes). CITY is cuba,
# queens or miami: the game starts straight in it, in a window of SIZE
# (default 1280x720). CFG is the text of `scripts/doctor_render.cfg` for the
# session with `;` between the lines, e.g. "draw_merge=1;draw_ab=8". The
# graphics module (RENDER_MODULE, default target/live/doctor_render.asi) and
# the file are put into SHADOW/scripts for the session and taken away after.
# STEPS.sh is sourced while the game runs; it may use:
#   bench STEP [ARG...]   a step of tools/game-bench.sh (shot, key, wait, until …)
#   live ARGS...          `doctor-toolkit live ARGS --game SHADOW`
#   wait_level [SECONDS]  wait until the module answers and the player stands in a level
#   merge_off / merge_on  `scripts/doctor_render_draws.off` (needs draw_trace=1)
# and the variables $shadow and $out. While the game runs the load of the
# machine goes to OUT/load.txt every 5 s: the load average, the processor
# share of everything but the game, and the graphics card. Afterwards
# OUT/render.log is the log of the graphics module, OUT/crash holds the crash
# reports written during the session and OUT/kernel.txt the lines of the
# kernel log about the graphics card that appeared during it.
# The whole session holds the lock of the shared bench (BENCH_LOCK).
set -euo pipefail
shadow=${1:?shadow}
out=${2:?output directory}
city=${3:?city: cuba, queens or miami}
cfg=${4:?text of doctor_render.cfg}
steps=${5:?steps script}
here=$(cd "$(dirname "$0")/.." && pwd)
t=${DOCTOR_TOOLKIT:-doctor-toolkit}
lock=${BENCH_LOCK:?BENCH_LOCK: the lock file of the shared bench}
module=${RENDER_MODULE:-$here/target/live/doctor_render.asi}
export SERVER=xvfb DXVK=${DXVK:-1} WINEDLLOVERRIDES=${WINEDLLOVERRIDES:-dinput8=n,b}
export BENCH_PREFIX=${BENCH_PREFIX:-$HOME/.local/share/doctor_toolkit/wine-gf2-bench-s101}
export SIZE=${SIZE:-1280x720}
# Decimal points whatever the locale (awk, printf).
export LC_NUMERIC=C
export GAME_OPTIONS="-windowed -resolution ${SIZE%x*} ${SIZE#*x} -stream gf2 $city"
unset DISPLAY

bench() { "$here/tools/game-bench.sh" do "$out" "$@"; }
live() { "$t" live "$@" --game "$shadow"; }
merge_off() { : >"$shadow/scripts/doctor_render_draws.off"; }
merge_on() { rm -f -- "$shadow/scripts/doctor_render_draws.off"; }
wait_level() {
    local limit=${1:-150} i
    for ((i = 0; i < limit; i += 3)); do
        if live player 2>/dev/null | grep -q .; then return 0; fi
        sleep 3
    done
    echo "bench_draws: the player did not appear in $limit s" >&2
    return 1
}

# Processor ticks: all, idle, and those of the processes of the game.
ticks() {
    local all idle game
    read -r all idle < <(awk '/^cpu /{s=0; for (i=2; i<=NF; i++) s+=$i; print s, $5+$6}' /proc/stat)
    game=$(for p in $(pgrep -f 'godfather2.exe|wineserver|Xvfb' || true); do
        awk '{print $14+$15}' "/proc/$p/stat" 2>/dev/null || true
    done | awk '{s+=$1} END {print s+0}')
    echo "$all $idle $game"
}
load_loop() {
    local a i g a2 i2 g2 cores
    cores=$(nproc)
    read -r a i g < <(ticks)
    while sleep 5; do
        read -r a2 i2 g2 < <(ticks)
        # In cores: everything that ran, the game's part, the rest.
        awk -v a=$((a2 - a)) -v i=$((i2 - i)) -v g=$((g2 - g)) -v n="$cores" -v l="$(cut -d' ' -f1 /proc/loadavg)" \
            -v gpu="$(nvidia-smi --query-gpu=utilization.gpu,temperature.gpu,memory.used --format=csv,noheader,nounits 2>&1 | head -1)" \
            'BEGIN {busy = (a - i) / a * n; game = g / a * n;
                    printf "%s load %s busy %.2f game %.2f others %.2f gpu %s\n", strftime("%H:%M:%S"), l, busy, game, busy - game, gpu}'
        a=$a2 i=$i2 g=$g2
    done
}
kernel_log() { journalctl -k -b --no-pager -o short-iso 2>/dev/null | grep -iE 'NVRM|Xid|nvidia' || true; }

mkdir -p "$out"
exec 9>"$lock"
flock 9
kernel_log >"$out/.kernel-before"
mkdir -p "$shadow/scripts"
cp -- "$module" "$shadow/scripts/doctor_render.asi"
printf '%s\r\n' "${cfg//;/$'\r\n'}" >"$shadow/scripts/doctor_render.cfg"
rm -f -- "$shadow/scripts/doctor_render.log" "$shadow/scripts/doctor_render_draws.off" "$shadow/scripts/doctor_render_trace.go"
stamp=$(mktemp)
"$here/tools/game-bench.sh" start "$shadow" "$out"
load_loop >"$out/load.txt" &
load_pid=$!
status=0
( set +e; source "$steps" ) || status=$?
kill "$load_pid" 2>/dev/null || true
"$here/tools/game-bench.sh" stop "$out" || true
cp -- "$shadow/scripts/doctor_render.log" "$out/render.log" 2>/dev/null || true
mkdir -p "$out/crash"
find "$shadow/_doctor_toolkit/crashes" -newer "$stamp" -type f -exec cp -t "$out/crash" -- {} + 2>/dev/null || true
find "$shadow/scripts" -maxdepth 1 -name 'doctor_render_trace_*.bin' -exec mv -t "$out" -- {} + 2>/dev/null || true
rm -f -- "$stamp" "$shadow/scripts/doctor_render.asi" "$shadow/scripts/doctor_render.cfg" \
    "$shadow/scripts/doctor_render.log" "$shadow/scripts/doctor_render_draws.off" "$shadow/scripts/doctor_render_trace.go"
kernel_log | diff "$out/.kernel-before" - | grep '^>' >"$out/kernel.txt" || true
rm -f -- "$out/.kernel-before"
nvidia-smi --query-gpu=name,utilization.gpu,temperature.gpu,memory.used --format=csv,noheader >"$out/gpu-after.txt" 2>&1 || true
flock -u 9
exit "$status"

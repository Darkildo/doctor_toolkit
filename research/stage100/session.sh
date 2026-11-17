#!/usr/bin/env bash
# One session of the game on the bench (run it under the bench lock):
#
#   flock $BENCH_LOCK research/stage100/session.sh OUT SHADOW CITY BODY [INSTALL_ARG...]
#
# OUT     folder for screenshots and logs (made anew)
# SHADOW  shadow copy of the game (mkshadow.sh)
# CITY    cuba | queens | miami: the game starts there (`-stream gf2 CITY`)
# BODY    bash file sourced once the level is loaded. Helpers:
#           L ARGS...     doctor-toolkit live ARGS
#           D STEP...     a step of the bench (shot NAME, hold w 3, ...)
#           shot NAME     screenshot OUT/NAME.png
#           tp X Y Z      teleport the player and wait a few frames
#           frames N      wait until the game has drawn N more frames
#           settle [N]    wait until the list of loaded zone streams stays
#                         the same for N polls (default 4, one per second);
#                         prints the seconds it took
#           log TEXT
# INSTALL_ARG  more arguments of `live install`: --apply --workspace-dir W
#         [--resources-in-memory [--resources-dump]], or --mods.
#
# Only Xvfb is used (never the user's display); the Wine prefix is the
# bench's own ($BENCH_PREFIX). Times are those of Wine + DXVK under Xvfb.
set -uo pipefail
. "$(dirname "$0")/env.sh"
OUT=$1
SH=$2
CITY=$3
BODY=$4
shift 4
W=${W:-1024}
H=${H:-768}
rm -rf "$OUT"
mkdir -p "$OUT"
cd "$ROOT"
unset DISPLAY WAYLAND_DISPLAY DBUS_SESSION_BUS_ADDRESS
export SERVER=xvfb DXVK=${DXVK:-1} SIZE=${W}x${H} WINEDLLOVERRIDES=dinput8=n,b
export SESSION_MAX=${SESSION_MAX:-1500}
now() { date +%s.%N; }
log() { echo "[$(date +%H:%M:%S)] $*" | tee -a "$OUT/run.log"; }
L() { "$T" live "$@" --game "$SH" --port "$PORT" --timeout 60; }
D() { tools/game-bench.sh do "$OUT" "$@" >>"$OUT/run.log" 2>&1; }
shot() { D shot "$1"; }
frame() { L status 2>/dev/null | sed -n 's/^main loop: GameMainLoop, frame \([0-9]*\).*/\1/p'; }
frames() {
    local a b
    a=$(frame)
    for _ in $(seq 1 120); do
        b=$(frame)
        [ $((${b:-0} - ${a:-0})) -ge "$1" ] && return
        sleep 0.5
    done
}
tp() {
    "$T" live teleport --yes --game "$SH" --port "$PORT" --timeout 60 -- "$1,$2,$3" >>"$OUT/run.log" 2>&1
    frames 6
}
# settle [N]: seconds until the set of loaded zone streams (…\\local\\…) is the
# same N times in a row, one poll a second; gives up after 90 polls.
settle() {
    local need=${1:-4} same=0 prev="" cur t0
    t0=$(now)
    for _ in $(seq 1 90); do
        # Zones only: streams of ambient animations come and go with the
        # people of the street and never settle.
        cur=$(L streams 2>/dev/null | grep -i '\\local\\' | sort | md5sum)
        if [ "$cur" = "$prev" ]; then same=$((same + 1)); else same=0; fi
        prev=$cur
        [ $same -ge "$need" ] && break
        sleep 1
    done
    echo "$(now) $t0 $need" | awk '{ printf "%.1f\n", $1 - $2 - $3 }'
}
crashed() {
    grep -q "Unhandled page fault\|Unhandled exception" "$OUT/wine.log" 2>/dev/null ||
        grep -q "status: exited" "$OUT/timeline.txt" 2>/dev/null
}
cleanup() {
    log "stopping"
    tools/game-bench.sh stop "$OUT" >>"$OUT/run.log" 2>&1
    cp "$SH/scripts/doctor_live.log" "$OUT/doctor_live.log" 2>/dev/null
    cp "$SH/scripts/doctor_live.cfg" "$OUT/doctor_live.cfg" 2>/dev/null
    ls "$SH/_doctor_toolkit/crashes" >"$OUT/crashes.txt" 2>/dev/null
    if [[ ${KEEP_STAGED:-0} != 1 ]]; then
        "$T" live uninstall "$SH" >>"$OUT/run.log" 2>&1
    fi
    tools/game-bench.sh verify "$GAME" "$SH" >"$OUT/verify.txt" 2>&1
    tail -2 "$OUT/verify.txt" | tee -a "$OUT/run.log"
    echo "wine.log exception lines: $(grep -ci 'Unhandled exception\|Unhandled page fault' "$OUT/wine.log" 2>/dev/null)" | tee -a "$OUT/run.log"
    log "done"
}
trap cleanup EXIT
log "install ($CITY, ${W}x${H}) $*"
"$T" live install "$SH" --writes --port "$PORT" "$@" \
    "--cmd-txt=-windowed -resolution $W $H -stream gf2 $CITY" >>"$OUT/run.log" 2>&1 || {
    log "install failed"
    tail -5 "$OUT/run.log"
    exit 1
}
log "start"
T_START=$(now)
tools/game-bench.sh start "$SH" "$OUT" >>"$OUT/run.log" 2>&1 || {
    log "start failed"
    tail -5 "$OUT/run.log"
    exit 1
}
ok=0
for i in $(seq 1 "${LOAD_TRIES:-300}"); do
    sleep 1
    if crashed; then
        log "the game crashed while loading"
        grep "Unhandled" "$OUT/wine.log" | head -3 | tee -a "$OUT/run.log"
        exit 3
    fi
    if L streams >"$OUT/streams0.txt" 2>/dev/null && [ "$(grep -c "$CITY" "$OUT/streams0.txt")" -gt 20 ]; then
        ok=1
        break
    fi
done
if [ $ok != 1 ]; then
    log "level did not load in time"
    L status >>"$OUT/run.log" 2>&1
    exit 2
fi
T_LEVEL=$(echo "$(now) $T_START" | awk '{ printf "%.1f", $1 - $2 }')
log "level streams loaded $T_LEVEL s after the start"
for j in $(seq 1 240); do
    f=$(frame)
    [ "${f:-0}" -ge "${FRAMES:-120}" ] && break
    sleep 0.5
done
T_FRAMES=$(echo "$(now) $T_START" | awk '{ printf "%.1f", $1 - $2 }')
log "frame ${f:-?} of the level $T_FRAMES s after the start"
echo "$T_LEVEL $T_FRAMES" >"$OUT/load_times.txt"
source "$BODY"

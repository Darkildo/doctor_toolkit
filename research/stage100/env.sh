# Stage 100 on the bench: common settings (source it).
#   ROOT     the checkout the programs are built from
#   GAME     the game folder (read only)
#   S        working folder: shadows, workspaces, bench output
# Programs: cargo build -p doctor-toolkit-cli; tools/build-live-module.sh;
# tools/build-crash-module.sh.
export ROOT=${ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)}
export GAME=${GAME:-${EARS_GF2_DIR:?set GAME or EARS_GF2_DIR to the game folder}}
export S=${S:?set S to a working folder outside the game and the repository}
# Shadows the game runs from: a short folder. The game builds "folder/stream
# path" in a buffer of 256 bytes (stage 77): a long folder crashes it on
# streams with long paths. Folders the game never runs from stay in $S.
export SHADOWS=${SHADOWS:-$S}
export T=${T:-$ROOT/target/debug/doctor-toolkit}
export BENCH_LOCK=${BENCH_LOCK:-$(dirname "$S")/bench.lock}
export BENCH_PREFIX=${BENCH_PREFIX:-$HOME/.local/share/doctor_toolkit/wine-gf2-bench-s100}
export PORT=${PORT:-47900}
# The toolkit's own settings and caches stay out of the user's home.
export DOCTOR_TOOLKIT_CONFIG_DIR=$S/config
export DOCTOR_TOOLKIT_CACHE_DIR=$S/cache
export DOCTOR_LIVE_MODULE=$ROOT/target/live/doctor_live.asi

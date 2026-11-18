# Stage 107 on the bench: settings (source it), on top of research/stage100/env.sh.
#   GAME     the game folder, edition 1.0.789.0 (read only)
#   S        working folder outside the game and the repository; short: the
#            game runs from $S/shadow (default /tmp/claude-1000/s107)
#   LOADER   an ASI loader (dinput8.dll) for the shadows, when the game has none
# Programs: cargo build -p doctor-toolkit-cli; tools/build-live-module.sh.
export ROOT=${ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)}
export S=${S:-/tmp/claude-1000/s107}
export BENCH_PREFIX=${BENCH_PREFIX:-$HOME/.local/share/doctor_toolkit/wine-gf2-bench-s107}
export PORT=${PORT:-47907}
# No crash module in the shadows: the state of a shadow is compared whole.
export CRASH=${CRASH:-0}
mkdir -p "$S"
. "$ROOT/research/stage100/env.sh"

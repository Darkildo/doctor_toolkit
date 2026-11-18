# Stage 110 on the bench: settings (source it), on top of research/stage100/env.sh.
#   GAME     the game folder, edition 1.0.789.0 (read only)
#   S        working folder outside the game and the repository; short: the
#            game runs from $S/shadow (default /tmp/claude-1000/s110)
#   LOADER   an ASI loader (dinput8.dll) for the shadows, when the game has none
# Programs: cargo build -p doctor-toolkit-cli; tools/build-live-module.sh.
export ROOT=${ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)}
export S=${S:-/tmp/claude-1000/s110}
export BENCH_PREFIX=${BENCH_PREFIX:-$HOME/.local/share/doctor_toolkit/wine-gf2-bench-s110}
export PORT=${PORT:-47910}
# No crash module in the shadows: the state of a shadow is compared whole.
export CRASH=${CRASH:-0}
mkdir -p "$S"
. "$ROOT/research/stage100/env.sh"
# The zone at the gate where the game starts in Havana and its SimGroup.
export ZONE=gf2/cuba/local/yuri/high/terrain3
export GROUP=150

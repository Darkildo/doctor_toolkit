#!/usr/bin/env bash
# A native mod with a new class of SimGroup objects (stages 80 and 81).
#
#   examples/native-class/build.sh OUT [GAME_COPY WORKSPACE [MOD.zip]]
#
# Builds into OUT/scripts the modules of the mod and, for those who copy by
# hand, the platform module:
#
#   doctor_example.asi     the mod in Rust (src/lib.rs): class DoctorCounter,
#                          Lua functions, a listener, a filter, a hook
#   doctor_example_c.asi   the mod in C (c/doctor_example_c.c): class
#                          DoctorCEcho and two Lua functions, from doctor_sdk.h
#   doctor_platform.asi    the platform module of the toolkit (also built
#                          into target/live with the ASI loader
#                          doctor_loader.dll, where the mod manager of this
#                          repository finds them)
#
# With GAME_COPY (a copy of the game or a shadow of `tools/game-bench.sh
# shadow`; nothing is written into it) and WORKSPACE it also makes the data of
# the mod, recorded in WORKSPACE: two objects of the new classes in the
# SimGroup of the level stream of Havana and a script that calls the new Lua
# functions (data/doctor_example.json). With MOD.zip the workspace and the two
# modules are built into one mod of format version 5 (section `native` of
# mod.json, docs/mods.md §1.6). The mod manager installs it — the code only
# after the player agrees:
#
#   doctor-toolkit mods install GAME MOD.zip         # shows the files and hashes
#   doctor-toolkit mods native GAME doctor-native-class --allow
#   doctor-toolkit mods apply GAME                   # data, then scripts/*.asi,
#                                                    # doctor_platform.asi and,
#                                                    # without a loader, dinput8.dll
#   doctor-toolkit mods unapply GAME                 # takes all of it away
#
#   doctor_example_count  x2 ─> DoctorCounter (limit 2) ─> doctor_example_fired
#   doctor_example_fired  ─> DoctorCEcho ─> doctor_example_ping ─> the listener
#   doctor_example_blocked ─> swallowed by the filter (the counter would reset)
#   doctor_example_query  ─> the script sends doctor_example_count through
#                             DoctorSend and prints DoctorCounterInfo()
#
# EXAMPLE_STREAM (default gf2/cuba/local) is the stream of the objects and the
# script; EXAMPLE_ZONE, when set, gets one more DoctorCounter. WORKSPACE must
# be new: the indices are taken from the files of GAME_COPY.
#
# What was checked in the game: docs/analysis/game-bench.md §43 (the modules
# copied by hand) and §45 (the mod installed by the mod manager). How it
# works: docs/analysis/asi-platform.md.
#
# Needs: rustup target add i686-pc-windows-gnu; i686-w64-mingw32-gcc.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
out=${1:?output directory}
game=${2:-}
ws=${3:-}
mod=${4:-}
t=${DOCTOR_TOOLKIT:-doctor-toolkit}
target=i686-pc-windows-gnu

mkdir -p "$out/scripts"
out=$(cd "$out" && pwd)

# 1. The platform module and the ASI loader of the toolkit, where the mod
#    manager looks for them; a copy of the platform for copying by hand.
"$root/tools/build-platform-module.sh" >/dev/null
"$root/tools/build-loader.sh" >/dev/null
cp "$root/target/live/doctor_platform.asi" "$out/scripts/doctor_platform.asi"

# 2. The mod in Rust.
CARGO_TARGET_I686_PC_WINDOWS_GNU_LINKER=i686-w64-mingw32-gcc \
    cargo build --release --target "$target" \
    --manifest-path "$here/Cargo.toml" \
    --target-dir "$root/target/example-native-class"
cp "$root/target/example-native-class/$target/release/doctor_example.dll" "$out/scripts/doctor_example.asi"

# 3. The mod in C, from the header of the SDK.
i686-w64-mingw32-gcc -std=c11 -Wall -Wextra -O2 -shared -static-libgcc \
    -I "$root/crates/doctor-toolkit-sdk/include" \
    -o "$out/scripts/doctor_example_c.asi" "$here/c/doctor_example_c.c"

echo "built: $out/scripts/doctor_platform.asi doctor_example.asi doctor_example_c.asi"
[[ -n $game ]] || exit 0
: "${ws:?workspace directory}"

# 4. The data. The toolkit learns the classes of the mod from its class file.
export DOCTOR_TOOLKIT_CLASSES=$here/classes.toml
"$t" class-schema "$here/classes.toml" >/dev/null

level=${EXAMPLE_STREAM:-gf2/cuba/local}
group=$("$t" list "$game/$level.str" | awk '$2 == "simgroup" { print $1; exit }')

# add CLASS -> index of the new object
add() {
    "$t" simgroup-add "$game" "$level" "$group" "$1" --workspace-dir "$ws" |
        sed -n 's/^added object \([0-9]*\).*/\1/p'
}
# Base.0 = 1: the creation flags every object of the game's logic has (the
# object manager reads them before it creates the object).
counter=$(add DoctorCounter)
"$t" simgroup-set "$game" "$level" "$group" "$counter" --workspace-dir "$ws" \
    --set Base.0=1 \
    --set DoctorCounter.count_on=doctor_example_count \
    --set DoctorCounter.send=doctor_example_fired \
    --set DoctorCounter.limit=2 \
    --set DoctorCounter.reset_on=doctor_example_blocked >/dev/null
echo=$(add DoctorCEcho)
"$t" simgroup-set "$game" "$level" "$group" "$echo" --workspace-dir "$ws" \
    --set Base.0=1 \
    --set DoctorCEcho.on=doctor_example_fired \
    --set DoctorCEcho.send=doctor_example_ping >/dev/null

# A third object in a stream of its own choice (EXAMPLE_ZONE, e.g. a zone that
# is loaded and unloaded as the player moves): sends doctor_example_zone each
# time doctor_example_count comes.
zone=${EXAMPLE_ZONE:-}
if [[ -n $zone ]]; then
    zone_group=$("$t" list "$game/$zone.str" | awk '$2 == "simgroup" { print $1; exit }')
    zone_counter=$("$t" simgroup-add "$game" "$zone" "$zone_group" DoctorCounter --workspace-dir "$ws" |
        sed -n 's/^added object \([0-9]*\).*/\1/p')
    "$t" simgroup-set "$game" "$zone" "$zone_group" "$zone_counter" --workspace-dir "$ws" \
        --set Base.0=1 \
        --set DoctorCounter.count_on=doctor_example_count \
        --set DoctorCounter.send=doctor_example_zone \
        --set DoctorCounter.limit=1 >/dev/null
    echo "data: DoctorCounter is object $zone_counter of $zone#$zone_group"
fi

# 5. The script that calls the new Lua functions.
"$t" script-deploy "$game" "$level" "$here/data/doctor_example.json" --workspace-dir "$ws" >/dev/null

echo "data: DoctorCounter is object $counter, DoctorCEcho object $echo of $level#$group; script doctor_example"

# 6. The mod: the data and the two modules. `--native-required`: without the
#    classes and the Lua functions the objects and the script are of no use.
if [[ -n $mod ]]; then
    "$t" mods build "$game" -o "$mod" --name "Doctor native class" --id doctor-native-class --mod-version 1.0 \
        --description "Objects of two new classes, a script and the code of the classes: the example native mod" \
        --native "$out/scripts/doctor_example.asi" \
        --native "$out/scripts/doctor_example_c.asi" \
        --native-required \
        --dir "$ws"
fi

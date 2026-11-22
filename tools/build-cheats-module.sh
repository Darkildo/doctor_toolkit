#!/usr/bin/env bash
# Builds doctor_cheats.asi, the developer menu for the player (stage 119 of
# PLAN-cheats.md), a native mod on doctor_platform.asi.
#
#   tools/build-cheats-module.sh [OUT.asi]            the module
#   tools/build-cheats-module.sh OUT.asi GAME MOD.zip also the mod for the
#                                                     mod manager
#
# OUT.asi defaults to target/live/doctor_cheats.asi. With GAME (a copy of the
# game, only read) and MOD.zip it writes a mod of format version 5 with the
# module in its `native` section and no data; the mod manager installs it,
# with doctor_platform.asi, after the player agrees:
#
#   doctor-toolkit mods install GAME MOD.zip
#   doctor-toolkit mods native GAME doctor-cheats --allow
#   doctor-toolkit mods apply GAME
#
# Settings: scripts/doctor_cheats.cfg next to the module (key, language,
# pad), see crates/doctor-toolkit-cheats-asi/src/config.rs.
#
# Needs: rustup target add i686-pc-windows-gnu; i686-w64-mingw32-gcc.
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
out=${1:-$root/target/live/doctor_cheats.asi}
game=${2:-}
mod=${3:-}
t=${DOCTOR_TOOLKIT:-doctor-toolkit}
target=i686-pc-windows-gnu

if ! rustup target list --installed 2>/dev/null | grep -qx "$target"; then
    echo "build-cheats-module: the Rust target $target is missing: rustup target add $target" >&2
    exit 1
fi
if ! command -v i686-w64-mingw32-gcc >/dev/null; then
    echo "build-cheats-module: the MinGW linker i686-w64-mingw32-gcc is missing (package gcc-mingw-w64-i686 or mingw32-gcc)" >&2
    exit 1
fi

cd "$root"
CARGO_TARGET_I686_PC_WINDOWS_GNU_LINKER=i686-w64-mingw32-gcc \
    cargo build --release --target "$target" \
    --manifest-path crates/doctor-toolkit-cheats-asi/Cargo.toml \
    --target-dir "$root/target/cheats-module"
mkdir -p "$(dirname "$out")"
cp "$root/target/cheats-module/$target/release/doctor_cheats.dll" "$out"
echo "$out"
[[ -n $game ]] || exit 0
: "${mod:?MOD.zip}"

# The mod has no data: an empty workspace of its own.
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
ws=$tmp/workspace
"$t" workspace init "$game" --dir "$ws" >/dev/null
version=$(sed -n 's/^version = "\(.*\)"/\1/p' crates/doctor-toolkit-cheats-asi/Cargo.toml | head -1)
"$t" mods build "$game" -o "$mod" --name "Doctor: developer menu" --id doctor-cheats \
    --mod-version "$version" --edition 1.0.789.0 \
    --description "A developer menu in the game (key ~ or Back + RB). Player: money, god mode, one hit and down, teleport to the waypoint of Don's view, the interface hidden, the hour of the day. Weapons: infinite ammo, no reload, damage, rate of fire and spread, no camera kick, the damage enemies deal. Crew and families come in later versions. Fonts: Oswald and Podkova, SIL Open Font License (the licences are inside the module)." \
    --native "doctor_cheats.asi=$out" \
    --dir "$ws"
echo "$mod"

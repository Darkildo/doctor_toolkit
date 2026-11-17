#!/usr/bin/env bash
# Builds the platform module of native mods (stage 80): a 32-bit Windows DLL
# that the ASI loader of the game loads (docs/analysis/asi-platform.md).
#
#   tools/build-platform-module.sh [OUT]
#
# OUT defaults to target/live/doctor_platform.asi. Put the file into the
# `scripts` folder of the game, next to the native mods (.asi files that
# export doctor_mod_init); examples/native-class/build.sh builds it together
# with an example mod.
#
# Needs the Rust target and a MinGW linker:
#   rustup target add i686-pc-windows-gnu
#   apt install gcc-mingw-w64-i686      # Debian, Ubuntu (i686-w64-mingw32-gcc)
#   dnf install mingw32-gcc             # Fedora
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
out=${1:-$root/target/live/doctor_platform.asi}
target=i686-pc-windows-gnu

if ! rustup target list --installed 2>/dev/null | grep -qx "$target"; then
    echo "build-platform-module: the Rust target $target is missing: rustup target add $target" >&2
    exit 1
fi
if ! command -v i686-w64-mingw32-gcc >/dev/null; then
    echo "build-platform-module: the MinGW linker i686-w64-mingw32-gcc is missing (package gcc-mingw-w64-i686 or mingw32-gcc)" >&2
    exit 1
fi

cd "$root"
CARGO_TARGET_I686_PC_WINDOWS_GNU_LINKER=i686-w64-mingw32-gcc \
    cargo build --release --target "$target" \
    --manifest-path crates/doctor-toolkit-platform-asi/Cargo.toml \
    --target-dir "$root/target/platform-module"
mkdir -p "$(dirname "$out")"
cp "$root/target/platform-module/$target/release/doctor_platform.dll" "$out"
echo "$out"

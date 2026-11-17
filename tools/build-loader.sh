#!/usr/bin/env bash
# Builds the ASI loader of the toolkit (stage 81): a 32-bit Windows DLL the
# mod manager installs into a game folder as dinput8.dll when the folder has
# no ASI loader and a mod with executable code needs one (docs/mods.md §1.6).
#
#   tools/build-loader.sh [OUT]
#
# OUT defaults to target/live/doctor_loader.dll, where the programs of this
# repository look for it; in a release the file lies next to the programs.
#
# Needs the Rust target and a MinGW linker:
#   rustup target add i686-pc-windows-gnu
#   apt install gcc-mingw-w64-i686      # Debian, Ubuntu (i686-w64-mingw32-gcc)
#   dnf install mingw32-gcc             # Fedora
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
out=${1:-$root/target/live/doctor_loader.dll}
target=i686-pc-windows-gnu

if ! rustup target list --installed 2>/dev/null | grep -qx "$target"; then
    echo "build-loader: the Rust target $target is missing: rustup target add $target" >&2
    exit 1
fi
if ! command -v i686-w64-mingw32-gcc >/dev/null; then
    echo "build-loader: the MinGW linker i686-w64-mingw32-gcc is missing (package gcc-mingw-w64-i686 or mingw32-gcc)" >&2
    exit 1
fi

cd "$root"
CARGO_TARGET_I686_PC_WINDOWS_GNU_LINKER=i686-w64-mingw32-gcc \
    cargo build --release --target "$target" \
    --manifest-path crates/doctor-toolkit-loader/Cargo.toml \
    --target-dir "$root/target/loader"
mkdir -p "$(dirname "$out")"
cp "$root/target/loader/$target/release/doctor_loader.dll" "$out"
echo "$out"

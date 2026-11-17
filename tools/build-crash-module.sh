#!/usr/bin/env bash
# Builds the in-game module of the crash handler (stage 99): a 32-bit Windows DLL
# that the ASI loader of the game loads (docs/analysis/crash-reports.md).
#
#   tools/build-crash-module.sh [OUT]
#
# OUT defaults to target/live/doctor_crash.asi; the toolkit finds it there
# when run from this repository (or set DOCTOR_CRASH_MODULE=PATH, or put the
# file next to the doctor-toolkit executable).
#
# Needs the Rust target and a MinGW linker:
#   rustup target add i686-pc-windows-gnu
#   apt install gcc-mingw-w64-i686      # Debian, Ubuntu (i686-w64-mingw32-gcc)
#   dnf install mingw32-gcc             # Fedora
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
out=${1:-$root/target/live/doctor_crash.asi}
target=i686-pc-windows-gnu

if ! rustup target list --installed 2>/dev/null | grep -qx "$target"; then
    echo "build-crash-module: the Rust target $target is missing: rustup target add $target" >&2
    exit 1
fi
if ! command -v i686-w64-mingw32-gcc >/dev/null; then
    echo "build-crash-module: the MinGW linker i686-w64-mingw32-gcc is missing (package gcc-mingw-w64-i686 or mingw32-gcc)" >&2
    exit 1
fi

cd "$root"
CARGO_TARGET_I686_PC_WINDOWS_GNU_LINKER=i686-w64-mingw32-gcc \
    cargo build --release --target "$target" \
    --manifest-path crates/doctor-toolkit-crash-asi/Cargo.toml \
    --target-dir "$root/target/crash-module"
mkdir -p "$(dirname "$out")"
cp "$root/target/crash-module/$target/release/doctor_crash.dll" "$out"
echo "$out"

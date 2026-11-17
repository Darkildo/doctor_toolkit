#!/usr/bin/env bash
# Builds the release package for Windows or macOS (Linux: tools/release.sh):
#   dist/doctor_toolkit-<version>-windows-x86_64.zip
#   dist/doctor_toolkit-<version>-macos-<arch>.tar.gz
# with doctor-toolkit, doctor-toolkit-gui, open-mod-manager, README,
# LICENSE, the documentation and BUILD-INFO.txt, plus a .sha256 file next to
# it, and open-mod-manager-<version>-<os>-<arch> (.zip/.tar.gz) with Open
# Mod Manager alone. Run on the system the package is for (the release workflow does);
# Git Bash on Windows.
#
# Usage: tools/package.sh [--skip-build]
#
# The binaries are built with the `dist` profile (thin LTO, one codegen unit,
# stripped). macOS binaries are not signed: Gatekeeper asks before the first
# start (or: xattr -d com.apple.quarantine doctor-toolkit* open-mod-manager).
set -euo pipefail

cd "$(dirname "$0")/.."
root=$(pwd)
target_dir=${CARGO_TARGET_DIR:-$root/target}
version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n1)
case "$(uname -s)" in
    MINGW* | MSYS* | CYGWIN*) os=windows exe=.exe arch=x86_64 ;;
    Darwin) os=macos exe='' arch=$(uname -m) ;;
    *) echo "package.sh is for Windows and macOS; on Linux use tools/release.sh" >&2; exit 1 ;;
esac
[[ $arch == arm64 ]] && arch=aarch64
name="doctor_toolkit-$version-$os-$arch"
bins=(doctor-toolkit doctor-toolkit-gui open-mod-manager)

if [[ ${1:-} != --skip-build ]]; then
    cargo build --profile dist --locked -p doctor-toolkit-cli -p doctor-toolkit-gui -p doctor-toolkit-mods
fi

stage=$(mktemp -d)
trap 'rm -rf -- "$stage"' EXIT
mkdir -p "$stage/$name/docs"
for bin in "${bins[@]}"; do
    cp "$target_dir/dist/$bin$exe" "$stage/$name/"
done
"$stage/$name/doctor-toolkit$exe" --version >/dev/null
cp README.md LICENSE "$stage/$name/"
cp -r docs/guide docs/formats docs/workspace.md docs/mods.md docs/navigation.md "$stage/$name/docs/"
cp docs/viewers.md docs/shell.md docs/model-import.md docs/blender.md docs/external-editors.md docs/python.md \
    "$stage/$name/docs/"

commit=$(git rev-parse --short HEAD 2>/dev/null || echo unknown)
cat >"$stage/$name/BUILD-INFO.txt" <<EOF
doctor_toolkit $version for $os $arch
commit: $commit
rustc: $(rustc --version)
The in-game modules (doctor_live.asi: the live link; doctor_render.asi: graphics; doctor_crash.asi: crash reports; doctor_platform.asi: the platform of native mods; doctor_loader.dll: the ASI loader the mod manager installs as dinput8.dll) are separate files of the release: put them next to the programs.
EOF
if [[ $os == macos ]]; then
    cat >>"$stage/$name/BUILD-INFO.txt" <<EOF
The programs are not signed: allow them in System Settings > Privacy & Security,
or run: xattr -d com.apple.quarantine doctor-toolkit doctor-toolkit-gui open-mod-manager
EOF
fi

mkdir -p dist
if [[ $os == windows ]]; then
    archive="dist/$name.zip"
    rm -f -- "$archive"
    (cd "$stage" && 7z a -tzip -mx=9 "$root/$archive" "$name" >/dev/null)
    (cd dist && sha256sum "$name.zip" >"$name.zip.sha256")
else
    archive="dist/$name.tar.gz"
    tar -C "$stage" -czf "$archive" "$name"
    (cd dist && shasum -a 256 "$name.tar.gz" >"$name.tar.gz.sha256")
fi
echo "built $archive ($(du -h "$archive" | cut -f1))"

# Open Mod Manager alone, for players: the program, LICENSE and a short README.
omm="open-mod-manager-$version-$os-$arch"
mkdir -p "$stage/$omm"
cp "$target_dir/dist/open-mod-manager$exe" LICENSE "$stage/$omm/"
cp tools/open-mod-manager-README.txt "$stage/$omm/README.txt"
if [[ $os == windows ]]; then
    rm -f -- "dist/$omm.zip"
    (cd "$stage" && 7z a -tzip -mx=9 "$root/dist/$omm.zip" "$omm" >/dev/null)
    (cd dist && sha256sum "$omm.zip" >"$omm.zip.sha256")
    echo "built dist/$omm.zip"
else
    tar -C "$stage" -czf "dist/$omm.tar.gz" "$omm"
    (cd dist && shasum -a 256 "$omm.tar.gz" >"$omm.tar.gz.sha256")
    echo "built dist/$omm.tar.gz"
fi

#!/usr/bin/env bash
# Builds the Linux release archive:
#   dist/doctor_toolkit-<version>-linux-x86_64.tar.gz
# with doctor-toolkit, doctor-toolkit-gui, doctor-toolkit-mods, README,
# LICENSE, the documentation, the menu entries with their icons and
# install-desktop.sh, plus a .sha256 file next to it. Next to the archive:
# the Python module as a wheel (when maturin is installed) and the Blender
# add-on as an extension package, with that wheel inside
# (dist/doctor_toolkit_blender-<version>-<platform>.zip); the archive has the
# add-on package too, in blender/.
#
# Usage: tools/release.sh [--skip-build]
#
# The binaries are built with the `dist` profile (thin LTO, one codegen unit,
# stripped). When cargo-zigbuild and zig are installed, they are linked
# against glibc $GLIBC (default 2.31, Ubuntu 20.04 / Debian 11) whatever the
# build machine has; otherwise against the glibc of the build machine.
# NATIVE=1 forces the native build.
#
# The dynamic dependencies are checked: only the C library and its
# companions may be linked at start-up, and for the editor the ALSA library
# (libasound.so.2, sound is on by default); graphics and window system
# libraries (X11, Wayland, Vulkan, OpenGL) are loaded at run time.
# The newest glibc symbol version required is printed and written into the
# archive as BUILD-INFO.txt, with the ALSA symbol versions the editor needs.
#
# Building needs the ALSA development files (libasound2-dev, alsa-lib-devel
# or alsa-lib): pkg-config finds libasound.so of the build machine and the
# editor is linked against it. Under cargo-zigbuild that library itself needs
# a newer glibc than the target; the linker is told not to check the
# undefined symbols of shared libraries (--allow-shlib-undefined), since at
# run time the user's own libasound comes with the user's glibc. Only the
# ALSA symbol versions the editor uses go into the binary; they are checked
# to be the ALSA_0.9 family, present in every alsa-lib since 2004.
set -euo pipefail

cd "$(dirname "$0")/.."
root=$(pwd)
target_dir=${CARGO_TARGET_DIR:-$root/target}
version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n1)
arch=$(uname -m)
name="doctor_toolkit-$version-linux-$arch"
bins=(doctor-toolkit doctor-toolkit-gui doctor-toolkit-mods)

packages=(-p doctor-toolkit-cli -p doctor-toolkit-gui -p doctor-toolkit-mods)
if [[ ${1:-} != --skip-build ]] && ! pkg-config --exists alsa; then
    echo "the ALSA development files are missing (pkg-config alsa): install libasound2-dev" \
        "(Debian, Ubuntu), alsa-lib-devel (Fedora) or alsa-lib (Arch)" >&2
    exit 1
fi
if [[ -z ${NATIVE:-} ]] && command -v cargo-zigbuild >/dev/null && command -v zig >/dev/null; then
    glibc_target=${GLIBC:-2.31}
    bin_dir="$target_dir/x86_64-unknown-linux-gnu/dist"
    if [[ ${1:-} != --skip-build ]]; then
        # libasound.so of the build machine needs its newer glibc; see above.
        CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUSTFLAGS="${CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUSTFLAGS:-} -C link-arg=-Wl,--allow-shlib-undefined" \
            cargo zigbuild --profile dist --locked --target "x86_64-unknown-linux-gnu.$glibc_target" \
            "${packages[@]}"
    fi
else
    bin_dir="$target_dir/dist"
    if [[ ${1:-} != --skip-build ]]; then
        cargo build --profile dist --locked "${packages[@]}"
    fi
fi

stage=$(mktemp -d)
trap 'rm -rf -- "$stage"' EXIT
mkdir -p "$stage/$name/docs"

# Libraries a binary may need at start-up. Anything else (libX11, libGL,
# libvulkan, libwayland, ...) must be opened at run time, so that the
# command line tool runs on a server without graphics; the editor may link
# the ALSA library too.
allowed='^(linux-vdso\.so|libc\.so|libm\.so|libgcc_s\.so|libpthread\.so|libdl\.so|librt\.so|ld-linux[-a-z0-9_]*\.so)'
max_glibc=""
alsa_versions=""
for bin in "${bins[@]}"; do
    path="$bin_dir/$bin"
    [[ -x $path ]] || { echo "missing $path; build first" >&2; exit 1; }
    "$path" --version >/dev/null
    while read -r lib; do
        if [[ $bin == doctor-toolkit-gui && $lib == libasound.so.2 ]]; then
            continue
        fi
        if [[ ! $lib =~ $allowed ]]; then
            echo "$bin links $lib at start-up; only the C library (and ALSA for the editor) is allowed" >&2
            exit 1
        fi
    done < <(ldd "$path" | awk '{print $1}' | sed 's|.*/||')
    if [[ $bin == doctor-toolkit-gui ]]; then
        alsa_versions=$(objdump -T "$path" | grep -o 'ALSA_[0-9A-Za-z.]*' | sort -uV | tr '\n' ' ' | sed 's/ $//')
        if [[ -z $alsa_versions ]]; then
            echo "$bin does not use ALSA: was it built without the audio feature?" >&2
            exit 1
        fi
        for v in $alsa_versions; do
            [[ $v == ALSA_0.9* ]] || { echo "$bin needs $v of libasound; only ALSA_0.9* is expected" >&2; exit 1; }
        done
    fi
    glibc=$(objdump -T "$path" | grep -o 'GLIBC_[0-9.]*' | sort -uV | tail -n1)
    max_glibc=$(printf '%s\n%s\n' "$max_glibc" "$glibc" | sed '/^$/d' | sort -V | tail -n1)
    cp "$path" "$stage/$name/"
done

cp README.md LICENSE "$stage/$name/"
# The in-game module of the live link (a Windows DLL the game's ASI loader
# loads) next to the programs, where the toolkit looks for it; skipped with
# a warning when the i686 Windows target or MinGW is missing.
if "$root/tools/build-live-module.sh" "$stage/$name/doctor_live.asi" >/dev/null; then
    echo "live module: doctor_live.asi"
else
    echo "warning: the live module was not built (see tools/build-live-module.sh)" >&2
fi
# Menu entries and icons: install-desktop.sh at the top finds the programs
# next to itself and the entries in desktop/; icons/ is for packagers.
cp tools/install-desktop.sh "$stage/$name/"
mkdir -p "$stage/$name/desktop"
cp tools/desktop/*.desktop "$stage/$name/desktop/"
"$bin_dir/doctor-toolkit-gui" --write-icons "$stage/$name/icons" >/dev/null
cp -r docs/guide docs/formats docs/workspace.md docs/mods.md docs/navigation.md "$stage/$name/docs/"
cp docs/viewers.md docs/shell.md docs/model-import.md docs/blender.md docs/external-editors.md docs/python.md \
    "$stage/$name/docs/"

# The Python module (abi3 wheel) and the Blender add-on with it.
mkdir -p dist "$stage/$name/blender"
wheels=()
maturin=${MATURIN:-maturin}
if command -v "$maturin" >/dev/null && [[ ${1:-} != --skip-build ]]; then
    wheel_args=(build --profile dist --strip -m crates/doctor-toolkit-py/Cargo.toml -o dist)
    if [[ -n ${glibc_target:-} ]]; then
        wheel_args+=(--zig --compatibility "manylinux_${glibc_target/./_}")
    fi
    env -u CONDA_PREFIX "$maturin" "${wheel_args[@]}" </dev/null
fi
for wheel in dist/doctor_toolkit-"$version"-*.whl; do
    [[ -f $wheel ]] && wheels+=(--wheel "$wheel")
done
if ((${#wheels[@]} == 0)); then
    echo "note: no wheel in dist/ (maturin not found); the Blender add-on is packed without the module" >&2
fi
addon=$(python3 tools/blender/build-extension.py -o dist "${wheels[@]}")
cp "$addon" "$stage/$name/blender/"

commit=$(git rev-parse --short HEAD 2>/dev/null || echo unknown)
dirty=$(git status --porcelain 2>/dev/null | grep -q . && echo " (with uncommitted changes)" || true)
cat >"$stage/$name/BUILD-INFO.txt" <<EOF
doctor_toolkit $version for Linux $arch
commit: $commit$dirty
rustc: $(rustc --version)
minimum glibc: ${max_glibc#GLIBC_}
doctor-toolkit-gui needs libasound.so.2 (ALSA) with: $alsa_versions
EOF

archive="dist/$name.tar.gz"
tar --sort=name --owner=0 --group=0 --numeric-owner -C "$stage" -czf "$archive" "$name"
(cd dist && sha256sum "$name.tar.gz" >"$name.tar.gz.sha256")

echo "built $archive ($(du -h "$archive" | cut -f1))"
echo "minimum glibc: ${max_glibc#GLIBC_}"
echo "libasound.so.2 symbol versions (editor): $alsa_versions"
echo "Blender add-on: $addon"
for bin in "${bins[@]}"; do
    echo "$bin: $(du -h "$bin_dir/$bin" | cut -f1), needs: $(ldd "$bin_dir/$bin" | awk '{print $1}' | sed 's|.*/||' | grep -v vdso | tr '\n' ' ')"
done

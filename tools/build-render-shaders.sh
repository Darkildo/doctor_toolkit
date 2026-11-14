#!/usr/bin/env bash
# Compiles the HLSL shaders of the graphics module (stage 59) to Direct3D 9
# bytecode: crates/doctor-toolkit-render-asi/shaders/*.hlsl -> *.cso. The
# module embeds the .cso files; they are in the repository, so building the
# module does not need this.
#
#   D3DCOMPILER=/path/to/d3dcompiler_47.dll tools/build-render-shaders.sh
#
# Needs the MinGW compiler (i686-w64-mingw32-gcc) and, on Linux, Wine. The
# compiler is D3DCompile of d3dcompiler_47.dll: a 32-bit one of Microsoft
# named by D3DCOMPILER (it ships with Windows 10 and many programs), else
# Wine's own. Wine runs in a prefix of its own and without a display.
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
shaders=$root/crates/doctor-toolkit-render-asi/shaders
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

command -v i686-w64-mingw32-gcc >/dev/null ||
    { echo "build-render-shaders: i686-w64-mingw32-gcc is missing" >&2; exit 1; }
i686-w64-mingw32-gcc -O2 -o "$work/compile.exe" "$root/tools/render-shaders/compile.c"
if [[ -n ${D3DCOMPILER:-} ]]; then
    cp "$D3DCOMPILER" "$work/d3dcompiler_47.dll"
fi
cp "$shaders"/*.hlsl "$work/"

run() {
    case $(uname -s) in
    MINGW* | MSYS* | CYGWIN*) (cd "$work" && ./compile.exe "$@") ;;
    *)
        (cd "$work" && DISPLAY= WAYLAND_DISPLAY= WINEDEBUG=-all \
            WINEPREFIX=${SHADER_WINEPREFIX:-$work/prefix} \
            WINEDLLOVERRIDES="d3dcompiler_47=n,b;winemenubuilder.exe=d" \
            wine compile.exe "$@")
        ;;
    esac
}

# name entry target
for spec in "post_vs main vs_3_0" "fxaa main ps_3_0" "cas main ps_3_0"; do
    read -r name entry target <<<"$spec"
    run "$name.hlsl" "$name.cso" "$entry" "$target"
    cp "$work/$name.cso" "$shaders/$name.cso"
    echo "$shaders/$name.cso ($(stat -c %s "$shaders/$name.cso") bytes, $target)"
done

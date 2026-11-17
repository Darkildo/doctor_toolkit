#!/usr/bin/env bash
# Screenshots of Open Mod Manager with a mod that carries executable code
# (stage 81, docs/analysis/game-bench.md §45), under an Xvfb of its own.
#
#   research/native_mod_shots.sh GAME WORK
#
# WORK has mod.zip of `research/bench_native_mod.sh setup` (the example of
# examples/native-class). Makes WORK/sh2, a shadow of GAME without the
# player's modules and loader, and writes WORK/shots/:
#
#   1-question.png      the mod is installed, nobody answered: the question
#   1-question-ru.png   the same in Russian
#   2-waiting.png       "Decide later": the card waits, the details offer
#                       the question again
#   3-agreed.png        allowed, not written yet
#   6-no-loader.png     … and neither the game nor the toolkit has a loader
#   4-installed.png     written into the game
#   5-other-edition.png the executable of the shadow is not 1.0.789.0
#
# Needs target/debug/doctor-toolkit and open-mod-manager of this repository,
# xvfb-run, and the files of the toolkit in target/live.
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
game=${1:?game folder}
work=$(cd "${2:?work folder}" && pwd)
t=$root/target/debug/doctor-toolkit
manager=$root/target/debug/open-mod-manager
shadow=$work/sh2
id=doctor-native-class
unset DISPLAY WAYLAND_DISPLAY

shot() {
    local name=$1
    shift
    xvfb-run -a -s "-screen 0 1600x1000x24" env "$@" \
        DOCTOR_TOOLKIT_MODS_SHOT="$work/shots/$name.png" DOCTOR_TOOLKIT_MODS_SIZE=1100x780 \
        "$manager" "$shadow" >/dev/null 2>&1 || echo "shot $name failed"
    ls -la "$work/shots/$name.png"
}

rm -rf "$shadow" "$work/shots" "$work/cfg-ru" "$work/cfg-en"
mkdir -p "$work/shots" "$work/cfg-ru" "$work/cfg-en"
printf 'language = "ru"\n' >"$work/cfg-ru/mods.toml"
printf 'language = "en"\n' >"$work/cfg-en/mods.toml"
"$root/tools/game-bench.sh" shadow "$game" "$shadow" gf2/cuba/local.str | tail -1
rm -f "$shadow/d3d9.dll" "$shadow"/scripts/doctor_live.* "$shadow"/scripts/GF2ASI.* "$shadow/dinput8.dll"
"$t" mods install "$shadow" "$work/mod.zip" | head -1

shot 1-question DOCTOR_TOOLKIT_CONFIG_DIR="$work/cfg-en"
shot 1-question-ru DOCTOR_TOOLKIT_CONFIG_DIR="$work/cfg-ru"
shot 2-waiting DOCTOR_TOOLKIT_CONFIG_DIR="$work/cfg-en" DOCTOR_TOOLKIT_MODS_ASK=later \
    DOCTOR_TOOLKIT_MODS_SELECT=$id
"$t" mods native "$shadow" "$id" --allow | tail -1
shot 3-agreed DOCTOR_TOOLKIT_CONFIG_DIR="$work/cfg-en" DOCTOR_TOOLKIT_MODS_SELECT=$id
# No loader in the game and none of the toolkit's to install.
shot 6-no-loader DOCTOR_TOOLKIT_CONFIG_DIR="$work/cfg-en" DOCTOR_TOOLKIT_MODS_SELECT=$id \
    DOCTOR_LOADER_MODULE=/nonexistent
"$t" mods apply "$shadow" | grep -c wrote
shot 4-installed DOCTOR_TOOLKIT_CONFIG_DIR="$work/cfg-en" DOCTOR_TOOLKIT_MODS_SELECT=$id

# Another edition: one byte of the link time in the PE header of the copy.
"$t" mods unapply "$shadow" >/dev/null
python3 - "$shadow/godfather2.exe" <<'EOF'
import struct, sys
path = sys.argv[1]
data = bytearray(open(path, "rb").read())
pe = struct.unpack_from("<I", data, 0x3C)[0]
data[pe + 8] ^= 1
open(path, "wb").write(data)
EOF
shot 5-other-edition DOCTOR_TOOLKIT_CONFIG_DIR="$work/cfg-en" DOCTOR_TOOLKIT_MODS_ASK=later \
    DOCTOR_TOOLKIT_MODS_SELECT=$id

#!/usr/bin/env bash
# Screenshots of the editor for README.md and the guide.
#
# Usage: tools/screenshots.sh GAME_DIR [OUT_DIR] [NAME...]
#
# Every shot starts doctor-toolkit-gui in its own Xvfb (memory only: no
# window on the desktop, no input from the user's session) with the
# screenshot presets of docs/viewers.md §5, and writes OUT_DIR/NAME.png
# (default docs/images). Without NAMEs all shots are taken. The editor binary
# is target/release/doctor-toolkit-gui (build it first) or $GUI; the
# interface language is UI_LANG (en, default, or ru). The game folder is
# shown as GAME_LINK (default /tmp/The Godfather II, a symbolic link made for
# the shots) so that no home folder appears in the pictures.
#
# The pictures show game data: they are for documentation of this tool only.
set -euo pipefail

here=$(dirname "$(realpath "$0")")
root=$(dirname "$here")
game=${1:?usage: tools/screenshots.sh GAME_DIR [OUT_DIR] [NAME...]}
out=${2:-$root/docs/images}
shift $(($# >= 2 ? 2 : 1))
gui=${GUI:-$root/target/release/doctor-toolkit-gui}
size=${SIZE:-1600x1000}
[[ -x $gui ]] || { echo "screenshots: $gui is not built (cargo build --release -p doctor-toolkit-gui)" >&2; exit 1; }
command -v Xvfb >/dev/null || { echo "screenshots: Xvfb is not installed" >&2; exit 1; }
mkdir -p "$out"
case ${UI_LANG:-en} in
    ru) locale=ru_RU.UTF-8 ;;
    *) locale=en_US.UTF-8 ;;
esac
link=${GAME_LINK:-/tmp/The Godfather II}
if [[ ! -e $link ]]; then
    ln -s -- "$(realpath "$game")" "$link"
fi
game=$link
# The index cache of the shots (its path shows in the log panel).
export XDG_CACHE_HOME=${SHOT_CACHE:-/tmp/doctor-toolkit-shots-cache}

# name | stream (or -) | environment of the shot (space-separated K=V)
# [| crop x,y,w,h in window pixels (needs python3 with Pillow)]
shots=(
  "overview|gf2/globalfolder/characters/allcharacters|DOCTOR_TOOLKIT_GUI_SELECT=5288"
  "model|gf2/globalfolder/characters/allcharacters|DOCTOR_TOOLKIT_GUI_SELECT=5288 DOCTOR_TOOLKIT_GUI_MODEL=window,yaw=0.6,pitch=0.15 DOCTOR_TOOLKIT_GUI_SHOT_FRAMES=150"
  "character|gf2/globalfolder/characters/allcharacters|DOCTOR_TOOLKIT_GUI_SELECT=6905 DOCTOR_TOOLKIT_GUI_CHARACTER=window,set=_all,lod=0,yaw=0.5"
  "level|-|DOCTOR_TOOLKIT_GUI_LEVEL=gf2/cuba/local;eye=305,9,395;look=283,2,371 DOCTOR_TOOLKIT_GUI_SHOT_FRAMES=200"
  "logic|-|DOCTOR_TOOLKIT_GUI_LOGIC=checkpoint:737 DOCTOR_TOOLKIT_GUI_SHOT_FRAMES=150"
  "logic-run|-|DOCTOR_TOOLKIT_GUI_LOGIC=checkpoint:737;run=load,send:cp_737_loaded,send:cp_737_was_started,wait:5,enter:#19;until=12 DOCTOR_TOOLKIT_GUI_SHOT_FRAMES=200"
  "cutscene|gf2/cuba/global/objectives/chapter60-mission08-killingcastro/cp_734/_734_igc_content|DOCTOR_TOOLKIT_GUI_SELECT=2 DOCTOR_TOOLKIT_GUI_CUTSCENE=time=6,load,zoom=30 DOCTOR_TOOLKIT_GUI_SHELL=max DOCTOR_TOOLKIT_GUI_SHOT_FRAMES=150|0,410,1200,325"
  "dialogue|gf2/globalfolder/dlg/03ccd0bc|DOCTOR_TOOLKIT_GUI_SELECT=15 DOCTOR_TOOLKIT_GUI_SHELL=max|0,390,1200,345"
  "movies|-|DOCTOR_TOOLKIT_GUI_MOVIES=FMV02:900"
  "wizards|-|DOCTOR_TOOLKIT_GUI_SHELL=welcome"
  "play|-|DOCTOR_TOOLKIT_GUI_SHELL=play|10,12,560,428"
  "anim-graph|gf2/globals_global|DOCTOR_TOOLKIT_GUI_SELECT=2657 DOCTOR_TOOLKIT_GUI_ANIM=graph=idle,node=2 DOCTOR_TOOLKIT_GUI_SHOT_FRAMES=60"
)

shoot() {
    local name=$1 stream=$2 envs=$3 crop=${4:-}
    local dfile
    dfile=$(mktemp)
    Xvfb -nolisten tcp -noreset -displayfd 3 -screen 0 "${size}x24" 3>"$dfile" >/dev/null 2>&1 </dev/null &
    local xvfb=$!
    local num=''
    for _ in $(seq 100); do
        num=$(head -1 "$dfile" 2>/dev/null || true)
        [[ -n $num ]] && break
        sleep 0.1
    done
    rm -f -- "$dfile"
    [[ -n $num ]] || { kill "$xvfb"; echo "screenshots: Xvfb did not start" >&2; return 1; }
    # The pointer away from the window: no hover highlights or tooltips.
    DISPLAY=":$num" xdotool mousemove 1590 990 2>/dev/null || true
    local -a args=("$game")
    [[ $stream == - ]] || args+=("$stream")
    local -a vars
    read -r -a vars <<<"$envs"
    rm -f -- "$out/$name.png"
    env -u WAYLAND_DISPLAY -u LANGUAGE DISPLAY=":$num" LC_ALL="$locale" LANG="$locale" DOCTOR_TOOLKIT_GUI_SHOT="$out/$name.png" "${vars[@]}" \
        timeout 300 "$gui" "${args[@]}" >"$out/.$name.log" 2>&1 </dev/null || true
    kill "$xvfb" 2>/dev/null
    wait "$xvfb" 2>/dev/null || true
    if [[ -s $out/$name.png ]]; then
        rm -f -- "$out/.$name.log"
        if [[ -n $crop ]]; then
            python3 -I -c '
import sys
from PIL import Image
path, (x, y, w, h) = sys.argv[1], map(int, sys.argv[2].split(","))
Image.open(path).crop((x, y, x + w, y + h)).save(path, optimize=True)
' "$out/$name.png" "$crop"
        fi
        echo "$name.png"
    else
        echo "screenshots: $name failed, see $out/.$name.log" >&2
    fi
}

for shot in "${shots[@]}"; do
    IFS='|' read -r name stream envs crop <<<"$shot"
    if (($# > 0)) && [[ " $* " != *" $name "* ]]; then
        continue
    fi
    shoot "$name" "$stream" "$envs" "$crop"
done

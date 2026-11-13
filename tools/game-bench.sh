#!/usr/bin/env bash
# In-game verification bench for The Godfather II under Wine.
#
# The game runs from a shadow copy of the game directory inside a nested X
# server (Xephyr) whose host window is never mapped: nothing appears on the
# desktop and no input reaches the host session. Clicks and key presses go to
# the nested display only. See docs/analysis/game-bench.md.
#
# Usage:
#   tools/game-bench.sh shadow GAME SHADOW [REL_PATH...]
#       (Re)builds SHADOW from GAME. Directories are real, large data files
#       (.str, .viv, .vp6, .vpat, > 16 MB) are symlinks, everything else and
#       every .toc is a copy. Each REL_PATH (e.g. gf2/frontend.str) becomes a
#       real copy too. Hashes of the originals go to SHADOW/.game-bench/.
#   tools/game-bench.sh verify GAME SHADOW
#       Checks that the originals of the copied files still have the hashes
#       recorded by `shadow`, and lists symlinks replaced by real files.
#   tools/game-bench.sh run SHADOW OUT
#       Starts the game from SHADOW, drives it with timed events (below) and
#       writes screenshots and logs to OUT.
#   tools/game-bench.sh play SHADOW OUT SCENARIO
#       Starts the game and runs the steps of SCENARIO, one per line (`#`
#       starts a comment), e.g. tools/bench/new-game.txt.
#   tools/game-bench.sh start SHADOW OUT
#       Starts the game and leaves it running (at most SESSION_MAX seconds,
#       default 3600) for interactive steps:
#   tools/game-bench.sh do OUT STEP [ARG...] [-- STEP [ARG...]]...
#   tools/game-bench.sh stop OUT
#   tools/game-bench.sh saves DEST
#       Copies the profiles and saves of the bench prefix to DEST; a later
#       run with SAVES=DEST starts with them.
#
# Steps (play, do):
#   shot [NAME]          screenshot OUT/NAME.png
#   key KEYSYM...        press keys one after another (HOLD each, GAP between)
#   hold KEYSYM SECONDS  hold a key, e.g. to walk
#   type TEXT            type letters, digits, spaces, - _ .
#   click X,Y            left click at game cursor position (see CLICK_MODE)
#   move X,Y             pointer move without a click
#   look DX,DY [N]       relative pointer motion N times: turns the camera
#   wait SECONDS         pause
#   until SECONDS        wait until SECONDS after the start of the game
#   ref NAME X,Y,W,H     save that area of the screen as reference NAME
#   await NAME [TIMEOUT] [TOLERANCE]
#                        wait until the area of reference NAME matches it
#                        (`!NAME`: until it no longer does); default 600 s, 12
#   find X,Y,Z [TOL]     MEMORY=1: find the player's world matrices at a known
#                        position (where a save puts him), see pos
#   track X,Y,Z [RADIUS] [KEY] [SECONDS]
#                        MEMORY=1: find the player's world matrices near
#                        X,Y,Z by walking (KEY, default w, 2 s)
#   walk KEY METRES [TIMEOUT]
#                        hold KEY until the player moved METRES, is blocked
#                        or TIMEOUT (40 s) passes (needs find or track)
#   pos                  the player's position and facing -> OUT/pos.txt
#                        "x y z dx dz yaw" (tools/bench/gamemem.py)
#   seek NAME KEYSYM [N] [TAP_SECONDS]
#                        tap KEYSYM until reference NAME matches (menus)
#   tools/game-bench.sh ahead OUT N [DY] [SIDE]
#       The point N m ahead of the player of OUT/pos.txt (DY up, SIDE to the
#       right) as X,Y,Z, e.g. for `level add --position`.
#   tools/game-bench.sh ref PNG NAME X,Y,W,H
#       Saves an area of a screenshot as reference NAME (in REFS).
#
# Environment:
#   DURATION      seconds before the game is stopped (default 90)
#   SHOTS         screenshot times in seconds, e.g. "20 40 60"
#                 (default: every 10 s)
#   CLICKS        "seconds:x,y" left clicks at game cursor position x,y
#   CLICK_MODE    relative (default): the game cursor follows relative
#                 motion, CURSOR_GAIN (default 2) game pixels per pointer
#                 pixel; absolute: plain pointer moves (dialogs, launchers)
#   KEYS          "seconds:keysym" key presses, e.g. "30:Return 32:Escape"
#   MOVES         "seconds:x,y" pointer moves without a click
#   HOLD          seconds a button or key is held (default 0.4; the game
#                 polls input once per frame, and software rendering is slow)
#   GAP           seconds between keys of one step (default 0.3)
#   TAP           seconds a key is held by `seek` (default 0.15)
#   REFS          references for await and seek
#                 (default ~/.local/share/doctor_toolkit/bench-refs)
#   MEMORY=1      start the game as a child of tools/bench/gamemem.py, which
#                 reads its memory (only reads) for find, track and pos
#   TRACE_FILES=1 log every file the game opens (strace) to OUT/files.trace;
#                 TRACE_SYSCALLS (default openat) widens it, e.g. openat,pread64
#   SESSION_MAX   seconds a `start` session may run (default 3600)
#   GAME_OPTIONS  the game's own launch options, written to SHADOW/cmd.txt
#                 for this run (the game ignores its command line and reads
#                 that file), e.g. "-stream gf2 cuba": straight into Havana
#                 without menus or saves, see docs/analysis/launch-flags.md;
#                 `doctor-toolkit play --stream … --dry-run` prints the line.
#                 A run without it takes away the cmd.txt of an earlier run
#   SAVES        directory made by `saves`: profiles and saves to start with
#                 (they replace those of the bench prefix)
#   SIZE        nested screen and Wine desktop size (default 1024x768)
#   BENCH_PREFIX  Wine prefix of the bench
#                 (default ~/.local/share/doctor_toolkit/wine-gf2-bench);
#                 created from BASE_PREFIX on first use, with audio off
#   BASE_PREFIX   default ~/.local/share/doctor_toolkit/wine-gf2
#   HOST_DISPLAY  host X display for Xephyr (default $DISPLAY)
#   SERVER=xvfb   nested server Xvfb (in memory, no host window at all; what
#                 the editor's game tab prefers) instead of Xephyr
#   VISIBLE=1     show the Xephyr window on the host (debugging only)
#   DXVK=1        draw with the graphics card: DXVK's 32-bit d3d9.dll (DXVK_DLL,
#                 default the newest of Lutris) in SHADOW for the run, with
#                 WINEDLLOVERRIDES d3d9=n,b and DXVK's log in OUT; taken
#                 away when the run ends. Needs 32-bit Vulkan of the card
#                 (NVIDIA: libnvidia-gl-<version>:i386). Without it the game
#                 draws with llvmpipe: 1-2 frames a second in a level
#   AUDIO_CAPTURE=1  turn the game's sound on for this run and record it to
#                 OUT/audio.wav: Wine's ALSA driver gets a private ALSA
#                 configuration whose only device is a file sink (no sound
#                 card, PulseAudio or PipeWire is opened)
set -euo pipefail
# Decimal points for the timing arithmetic, but keep the character set: Wine
# converts file names with it, and under LC_ALL=C the game cannot create its
# profile directory "EA Games/The Godfather™ II" ("Unable to find your profile").
unset LC_ALL
export LC_NUMERIC=C
if [[ $(locale charmap 2>/dev/null) != UTF-8 ]]; then
    export LC_CTYPE=C.UTF-8
fi

here=$(dirname "$(realpath "$0")")
# The user's home: Wine later runs with the home of the bench prefix.
user_home=$HOME

die() {
    echo "game-bench: $*" >&2
    exit 1
}

# Files that stay symlinks unless named explicitly.
is_bulk() {
    local path=$1 lower=${1,,}
    case $lower in
    *.str | *.viv | *.vp6 | *.vpat) return 0 ;;
    esac
    (($(stat -c %s "$path") > 16 * 1024 * 1024))
}

cmd_shadow() {
    (($# >= 2)) || die "usage: shadow GAME SHADOW [REL_PATH...]"
    local game shadow
    game=$(realpath "$1")
    shadow=$(realpath -m "$2")
    shift 2
    [[ -f $game/godfather2.exe ]] || die "$game is not a game directory"
    case $shadow/ in
    "$game"/*) die "shadow must not be inside the game directory" ;;
    esac
    if [[ -e $shadow ]]; then
        [[ -f $shadow/.game-bench/shadow ]] || die "$shadow exists and is not a shadow directory"
        # rm does not follow symlinks: only the shadow's own entries go.
        rm -rf -- "$shadow"
    fi
    mkdir -p "$shadow/.game-bench"
    echo "$game" >"$shadow/.game-bench/shadow"

    local copied=0 linked=0 rel
    while IFS= read -r -d '' rel; do
        rel=${rel#./}
        if [[ -d $game/$rel ]]; then
            mkdir -p "$shadow/$rel"
        elif [[ ${rel,,} != *.toc ]] && is_bulk "$game/$rel"; then
            ln -s "$game/$rel" "$shadow/$rel"
            linked=$((linked + 1))
        else
            cp -p "$game/$rel" "$shadow/$rel"
            copied=$((copied + 1))
        fi
    done < <(cd "$game" && find . -mindepth 1 ! -path './_doctor_toolkit' ! -path './_doctor_toolkit/*' -print0)

    for rel in "$@"; do
        [[ -f $game/$rel ]] || die "$game/$rel does not exist"
        rm -f -- "$shadow/$rel"
        cp -p "$game/$rel" "$shadow/$rel"
        copied=$((copied + 1))
    done

    # Hashes of the originals of every file that may be written in the shadow.
    (
        cd "$game"
        { find . -iname '*.toc' -printf '%P\n'; (($# == 0)) || printf '%s\n' "$@"; } | sort -u |
            while IFS= read -r rel; do sha256sum -- "$rel"; done
    ) >"$shadow/.game-bench/originals.sha256"
    echo "shadow $shadow: $copied copied, $linked symlinked"
    echo "hashes of originals: $shadow/.game-bench/originals.sha256"
}

cmd_verify() {
    (($# == 2)) || die "usage: verify GAME SHADOW"
    local game shadow
    game=$(realpath "$1")
    shadow=$(realpath "$2")
    (cd "$game" && sha256sum --quiet -c "$shadow/.game-bench/originals.sha256") &&
        echo "originals unchanged: $(wc -l <"$shadow/.game-bench/originals.sha256") files"
    # Bulk files that are no longer symlinks were rewritten in the shadow.
    (cd "$shadow" && find . -type f \( -iname '*.str' -o -iname '*.viv' \) -printf 'real copy in shadow: %P\n')
}

# Replaces every symbolic link under drive_c of PREFIX that leads out of the
# prefix with an empty directory. Wine links the user folders (Documents,
# Desktop, Templates, ...) to the host home. Prints the links it replaced.
sanitize_prefix() {
    local prefix link target
    prefix=$(realpath "$1")
    while IFS= read -r -d '' link; do
        target=$(realpath -m -- "$link")
        case $target/ in
        "$prefix"/*) continue ;;
        esac
        echo "replaced link $link -> $(readlink -- "$link")"
        rm -- "$link"
        mkdir -- "$link"
    done < <(find "$prefix/drive_c" -type l -print0)
}

# A home directory inside the bench prefix for every Wine command of the
# bench, with XDG user directories inside it. GF2 asks Wine for its profile
# folder with SHGetFolderPathAndSubDir(CSIDL_PERSONAL | CSIDL_FLAG_CREATE),
# and Wine then creates that folder (Documents/EA Games/<game>/<profile id>)
# as a link to the XDG documents directory of $HOME: with the real home the
# game reads and writes the player's own saves in ~/Documents. Wine commands
# get HOME and XDG_CONFIG_HOME from `wine_env`.
bench_home() {
    local home=$1/bench-home dir
    mkdir -p "$home/.config"
    local -a dirs=(DESKTOP:Desktop DOCUMENTS:Documents DOWNLOAD:Downloads MUSIC:Music
        PICTURES:Pictures VIDEOS:Videos TEMPLATES:Templates PUBLICSHARE:Public)
    : >"$home/.config/user-dirs.dirs"
    for dir in "${dirs[@]}"; do
        mkdir -p "$home/${dir#*:}"
        echo "XDG_${dir%%:*}_DIR=\"$home/${dir#*:}\"" >>"$home/.config/user-dirs.dirs"
    done
}

# Environment for Wine commands of PREFIX (as `env` arguments).
wine_env() {
    echo "WINEPREFIX=$1" "HOME=$1/bench-home" "XDG_CONFIG_HOME=$1/bench-home/.config"
}

# Creates the bench prefix: a copy of the base prefix that
# - keeps profiles and saves inside the prefix: Wine links Documents, Desktop
#   and so on to the host home, where the game would read and write the
#   player's real profiles;
# - has audio off;
# - reports an NVIDIA adapter: under software rendering (llvmpipe) the game
#   stops with "Your Graphics Card does not meet ... minimum requirements".
ensure_prefix() {
    local prefix=$1 base=$2
    if [[ -d $prefix ]]; then
        sanitize_prefix "$prefix"
        bench_home "$prefix"
        return
    fi
    [[ -d $base ]] || die "base prefix $base does not exist"
    echo "creating bench prefix $prefix from $base"
    mkdir -p "$(dirname "$prefix")"
    cp -a "$base" "$prefix"
    sanitize_prefix "$prefix"
    bench_home "$prefix"
    local -a env_args
    read -r -a env_args <<<"$(wine_env "$prefix")"
    local -a reg=(env "${env_args[@]}" WINEDEBUG=-all wine reg add)
    {
        "${reg[@]}" 'HKCU\Software\Wine\Drivers' /v Audio /d '' /f
        "${reg[@]}" 'HKCU\Software\Wine\Direct3D' /v VideoPciVendorID /t REG_DWORD /d 0x10de /f
        "${reg[@]}" 'HKCU\Software\Wine\Direct3D' /v VideoPciDeviceID /t REG_DWORD /d 0x0402 /f
        "${reg[@]}" 'HKCU\Software\Wine\Direct3D' /v VideoMemorySize /d 1024 /f
    } </dev/null >/dev/null 2>&1
    WINEPREFIX=$prefix wineserver -w
}

# State of one game run, shared by run, play and the session commands.
shadow='' out='' size='' prefix='' base='' host=''
parent_pid='' xephyr_pid='' game_pid='' guard_pid='' start='' display=''

# Reads the settings of a run and prepares the prefix.
setup() {
    shadow=$(realpath "$1")
    out=$(realpath -m "$2")
    [[ -f $shadow/.game-bench/shadow ]] || die "$shadow is not a shadow directory"
    mkdir -p "$out"
    size=${SIZE:-1024x768}
    prefix=${BENCH_PREFIX:-$HOME/.local/share/doctor_toolkit/wine-gf2-bench}
    base=${BASE_PREFIX:-$HOME/.local/share/doctor_toolkit/wine-gf2}
    host=${HOST_DISPLAY:-${DISPLAY:-}}
    [[ -n $host ]] || die "no host display: set HOST_DISPLAY"
    [[ $prefix != "$base" ]] || die "the bench prefix must differ from the base prefix"
    ensure_prefix "$prefix" "$base"
    prefix=$(realpath "$prefix")
    if [[ -n ${SAVES:-} ]]; then
        load_saves "$SAVES"
    fi
}

# Directories of the bench prefix where the game keeps profiles and saves.
# They must be real directories: in a plain Wine prefix they are links to the
# host home, where the player's own profiles live.
save_dirs() {
    local user
    user=$(id -un)
    local dir
    for dir in "$prefix/drive_c/users/$user/Documents" \
        "$prefix/drive_c/users/$user/AppData/Local"; do
        [[ -d $dir && ! -L $dir ]] || die "$dir is not a real directory of the bench prefix"
        echo "$dir"
    done
}

# Replaces the profiles and saves of the bench prefix with those in DIR
# (made by `saves`): DIR/Documents/EA Games and DIR/Local/EA Games.
load_saves() {
    local from
    from=$(realpath "$1")
    [[ -d $from/Documents ]] || die "$from has no Documents directory (see \`saves\`)"
    [[ -z $(find "$from" -type l) ]] || die "$from contains symbolic links; refusing to copy it"
    local documents local_dir
    { read -r documents && read -r local_dir; } < <(save_dirs)
    rm -rf -- "$documents/EA Games" "$local_dir/EA Games"
    cp -a -- "$from/Documents/EA Games" "$documents/"
    if [[ -d "$from/Local/EA Games" ]]; then
        cp -a -- "$from/Local/EA Games" "$local_dir/"
    fi
}

cleanup() {
    set +e
    if [[ -n $guard_pid ]]; then
        kill "$guard_pid" 2>/dev/null
        wait "$guard_pid" 2>/dev/null
        guard_pid=''
    fi
    # Only the processes of the bench prefix are stopped.
    WINEPREFIX=$prefix wineserver -k 2>/dev/null
    if [[ -n $game_pid ]]; then
        for _ in 1 2 3 4 5 6 7 8 9 10; do
            # A child that has exited stays a zombie until waited for.
            [[ $(ps -o stat= -p "$game_pid" 2>/dev/null) == Z* ]] && break
            kill -0 "$game_pid" 2>/dev/null || break
            sleep 1
        done
        kill -9 "$game_pid" 2>/dev/null
        wait "$game_pid" 2>/dev/null
    fi
    WINEPREFIX=$prefix wineserver -k9 2>/dev/null
    dxvk_remove
    [[ -n $xephyr_pid ]] && kill "$xephyr_pid" 2>/dev/null && wait "$xephyr_pid" 2>/dev/null
    [[ -n $parent_pid ]] && kill "$parent_pid" 2>/dev/null && wait "$parent_pid" 2>/dev/null
    game_pid='' xephyr_pid='' parent_pid=''
    return 0
}

# Starts the nested X server and the game. Sets display, game_pid, start.
launch() {
    # Nested X server. Without VISIBLE its host window is an unmapped window.
    # `-parent` must come before `-screen`: after it, Xephyr makes a second
    # screen for the parent and shows the first one (the one the game uses)
    # in a window of its own on the host display.
    local -a xephyr_args=(-no-host-grab -nolisten tcp -noreset -displayfd 3)
    local server=Xephyr
    if [[ ${SERVER:-xephyr} == xvfb ]]; then
        # Xvfb draws into memory only: no host window, no host display.
        server=Xvfb
        xephyr_args=(-nolisten tcp -noreset -displayfd 3 -screen 0 "${size}x24")
    elif [[ ${VISIBLE:-0} != 1 ]]; then
        local fifo=$out/.parent
        HOST_DISPLAY=$host "$here/hidden-parent.py" "${size%x*}" "${size#*x}" \
            >"$fifo" 2>"$out/parent.log" </dev/null &
        parent_pid=$!
        local parent=''
        for _ in $(seq 50); do
            parent=$(head -1 "$fifo" 2>/dev/null || true)
            [[ -n $parent ]] && break
            sleep 0.1
        done
        rm -f -- "$fifo"
        [[ -n $parent ]] || die "could not create the host window, see $out/parent.log"
        xephyr_args+=(-parent "$parent")
    fi
    [[ $server == Xvfb ]] || xephyr_args+=(-screen "$size")
    local dfile=$out/.display
    : >"$dfile"
    DISPLAY=$host "$server" "${xephyr_args[@]}" 3>"$dfile" >"$out/xephyr.log" 2>&1 </dev/null &
    xephyr_pid=$!
    local num=''
    for _ in $(seq 100); do
        num=$(head -1 "$dfile" 2>/dev/null || true)
        [[ -n $num ]] && break
        sleep 0.1
    done
    rm -f -- "$dfile"
    [[ -n $num ]] || die "$server did not start, see $out/xephyr.log"
    display=:$num
    echo "nested display $display ($size, $server)" | tee "$out/timeline.txt"

    # Everything below talks to the nested display only.
    export DISPLAY=$display
    # The host display is no longer needed: from here on Wine runs with the
    # home of the prefix (see bench_home).
    local -a env_args
    read -r -a env_args <<<"$(wine_env "$prefix")"
    export "${env_args[@]}" WINEARCH=win32
    export WINEDEBUG=${WINEDEBUG:-err+all,fixme-all}
    unset WAYLAND_DISPLAY

    # Sound: off, or captured to a file through a private ALSA configuration.
    # The configuration replaces the system one entirely, so the default
    # device cannot be routed to a real card or a sound server.
    local audio_driver=''
    if [[ ${AUDIO_CAPTURE:-0} == 1 ]]; then
        audio_driver=alsa
        cat >"$out/asound.conf" <<EOF
pcm.!default {
    type file
    slave.pcm "bench_null"
    file "$out/audio.wav"
    format "wav"
}
pcm.bench_null {
    type null
}
EOF
        export ALSA_CONFIG_PATH=$out/asound.conf
        export PULSE_SERVER=unix:/nonexistent PIPEWIRE_REMOTE=/nonexistent
        rm -f -- "$out/audio.wav"
    fi
    wine reg add 'HKCU\Software\Wine\Drivers' /v Audio /d "$audio_driver" /f >/dev/null 2>&1 </dev/null

    # A Wine virtual desktop of the nested screen size: mode changes of the
    # game stay inside it and its windows need no window manager.
    wine reg add 'HKCU\Software\Wine\Explorer' /v Desktop /d Default /f >/dev/null 2>&1 </dev/null
    wine reg add 'HKCU\Software\Wine\Explorer\Desktops' /v Default /d "$size" /f >/dev/null 2>&1 </dev/null
    # Bring up the desktop once before the game: without this the first start
    # in a new prefix has shown the "Graphics Card does not meet" error.
    wine cmd /c exit >/dev/null 2>&1 </dev/null || true
    wineserver -w
    game_options
    dxvk_setup
    local -a trace=()
    if [[ ${TRACE_FILES:-0} == 1 ]]; then
        # Every file the game opens, to learn which streams a level loads.
        trace=(strace -f -qq -e "trace=${TRACE_SYSCALLS:-openat}" -e signal=none -o "$out/files.trace")
    fi
    local -a memory=()
    if [[ ${MEMORY:-0} == 1 ]]; then
        # The game as a child of a reader of its memory (see track, pos).
        memory=(python3 "$here/bench/gamemem.py" serve "$out/mem.sock" --)
    fi
    (cd "$shadow" && exec "${trace[@]}" "${memory[@]}" wine godfather2.exe) >"$out/wine.log" 2>&1 </dev/null &
    game_pid=$!
    start=$(date +%s.%N)
    guard_links &
    guard_pid=$!
}

# Takes away the d3d9.dll the bench put into the shadow (DXVK=1); one
# someone changed since stays.
dxvk_remove() {
    [[ -n $shadow ]] || return 0
    local file=$shadow/d3d9.dll mark=$shadow/.game-bench/d3d9.dll
    if [[ -f $mark ]]; then
        cmp -s -- "$mark" "$file" && rm -f -- "$file"
        rm -f -- "$mark"
    fi
    return 0
}

# DXVK=1: DXVK's d3d9.dll in the shadow for this run.
dxvk_setup() {
    dxvk_remove
    [[ ${DXVK:-0} == 1 ]] || return 0
    local file=$shadow/d3d9.dll mark=$shadow/.game-bench/d3d9.dll dll=${DXVK_DLL:-}
    if [[ -z $dll ]]; then
        dll=$(printf '%s\n' "$user_home"/.local/share/lutris/runtime/dxvk/*/x32/d3d9.dll | sort -V | tail -1)
    fi
    [[ -f $dll ]] || die "DXVK=1: no DXVK d3d9.dll found, set DXVK_DLL"
    [[ ! -e $file && ! -L $file ]] || die "$file exists and was not written by the bench"
    cp -- "$dll" "$file"
    cp -- "$file" "$mark"
    export WINEDLLOVERRIDES="${WINEDLLOVERRIDES:+$WINEDLLOVERRIDES;}d3d9=n,b"
    export DXVK_LOG_PATH=${DXVK_LOG_PATH:-$out}
    echo "dxvk: $dll" >>"$out/timeline.txt"
}

# The game's launch options (GAME_OPTIONS) in SHADOW/cmd.txt, the only place
# the game reads them from. The file of an earlier run is taken away when
# GAME_OPTIONS is not set; a cmd.txt the bench did not write is never touched.
game_options() {
    local file=$shadow/cmd.txt mark=$shadow/.game-bench/cmd.txt
    # A cmd.txt that differs from the bench's (e.g. `doctor-toolkit play
    # --stream` replaced it for this run and puts it back afterwards) stays,
    # and so does the mark, so that a later run still takes the bench's away.
    if [[ -f $mark ]]; then
        if cmp -s -- "$mark" "$file"; then
            rm -f -- "$file" "$mark"
        elif [[ ! -e $file ]]; then
            rm -f -- "$mark"
        fi
    fi
    [[ -n ${GAME_OPTIONS:-} ]] || return 0
    [[ ! -e $file && ! -L $file ]] || die "$file exists and was not written by the bench"
    printf '%s\r\n' "$GAME_OPTIONS" >"$file"
    cp -- "$file" "$mark"
    echo "game options: $GAME_OPTIONS" >>"$out/timeline.txt"
}

# Every second while the game runs: a symbolic link under drive_c that leads
# out of the prefix stops the game at once (before it can write through it),
# is replaced by an empty directory and is reported in the timeline.
guard_links() {
    local replaced
    while kill -0 "$game_pid" 2>/dev/null; do
        if [[ -n $(find "$prefix/drive_c" -type l -lname '/*' ! -lname "$prefix/*" -print -quit) ]] ||
            [[ -n $(find "$prefix/drive_c" -type l -lname '[!/]*' -print -quit) ]]; then
            replaced=$(sanitize_prefix "$prefix")
            if [[ -n $replaced ]]; then
                WINEPREFIX=$prefix wineserver -k 2>/dev/null
                echo "WARNING: game stopped, $replaced" >>"$out/timeline.txt"
                : >"$out/LINK_ESCAPE"
                return
            fi
        fi
        sleep 1
    done
}

elapsed() {
    awk -v a="$(date +%s.%N)" -v b="$start" 'BEGIN { printf "%.1f", a - b }'
}

grab() {
    ffmpeg -loglevel error -y -f x11grab -video_size "$size" -i "$display" \
        -frames:v 1 "$1" </dev/null || true
}

press() {
    xdotool keydown "$1" && sleep "$2" && xdotool keyup "$1" || true
}

# Keysym of one character for `type`.
keysym_of() {
    case $1 in
    [a-zA-Z0-9]) echo "$1" ;;
    ' ') echo space ;;
    '-') echo minus ;;
    '_') echo underscore ;;
    '.') echo period ;;
    *) die "type: no key for character '$1'" ;;
    esac
}

# Left click at game cursor position x,y (see CLICK_MODE).
click_at() {
    local xy=$1
    if [[ ${CLICK_MODE:-relative} == relative ]]; then
        # The game reads relative motion (DirectInput) and Wine warps the
        # pointer back to the centre: push the game cursor into the top left
        # corner, then move it by the target / gain.
        local cx=$((${size%x*} / 2)) cy=$((${size#*x} / 2)) gain=${CURSOR_GAIN:-2}
        xdotool mousemove 0 0 && sleep 0.3
        xdotool mousemove "$((cx + ${xy%,*} / gain))" "$((cy + ${xy#*,} / gain))" && sleep 0.3
    else
        xdotool mousemove "${xy%,*}" "${xy#*,}" && sleep 0.2
    fi
    xdotool mousedown 1 && sleep "${HOLD:-0.4}" && xdotool mouseup 1 || true
}

# Runs one step against the nested display; see the usage at the top.
step() {
    local kind=$1
    shift
    local note=''
    case $kind in
    shot)
        local name=${1:-shot_$(elapsed)s}
        grab "$out/$name.png"
        note="pointer $(xdotool getmouselocation 2>/dev/null | cut -d' ' -f1-2)"
        ;;
    key)
        local k
        for k in "$@"; do
            press "$k" "${HOLD:-0.4}"
            sleep "${GAP:-0.3}"
        done
        ;;
    hold)
        (($# == 2)) || die "usage: hold KEYSYM SECONDS"
        press "$1" "$2"
        ;;
    type)
        local text="$*" i
        for ((i = 0; i < ${#text}; i++)); do
            press "$(keysym_of "${text:i:1}")" "${HOLD:-0.4}"
            sleep "${GAP:-0.3}"
        done
        ;;
    click) click_at "$1" ;;
    move) xdotool mousemove "${1%,*}" "${1#*,}" || true ;;
    look)
        # Relative pointer motion: turns the camera in game.
        local n=${2:-1} j
        for ((j = 0; j < n; j++)); do
            xdotool mousemove_relative -- "${1%,*}" "${1#*,}" || true
            sleep 0.1
        done
        ;;
    wait) sleep "$1" ;;
    until)
        # Absolute time since the start of the game.
        while awk -v e="$(elapsed)" -v a="$1" 'BEGIN { exit !(e < a) }'; do
            kill -0 "$game_pid" 2>/dev/null || break
            sleep 0.2
        done
        ;;
    ref)
        (($# == 2)) || die "usage: ref NAME X,Y,W,H"
        save_ref "$1" "$2"
        ;;
    await)
        (($# >= 1)) || die "usage: await NAME [TIMEOUT] [TOLERANCE]"
        local result=0
        await_ref "$@" || result=$?
        if ((result)); then
            step shot "await-$1-timeout"
            echo "$(elapsed)s await $* timed out" >>"$out/timeline.txt"
            return 1
        fi
        note="matched"
        ;;
    seek)
        # Presses KEYSYM until reference NAME matches, e.g. to select a menu
        # item: a held key repeats in menus, so it is only tapped (TAP
        # seconds, or TAP_SECONDS: the pause menu over the running level
        # sees a key only when it is held longer, ~0.7 s).
        (($# >= 2)) || die "usage: seek NAME KEYSYM [MAX] [TAP_SECONDS]"
        local tries=${3:-10} tap=${4:-${TAP:-0.15}} t found=0
        for ((t = 0; t <= tries; t++)); do
            if await_ref "$1" 0; then
                found=1
                break
            fi
            if ((t < tries)); then
                press "$2" "$tap"
                sleep 1.5
            fi
        done
        if ((!found)); then
            step shot "seek-$1-failed"
            return 1
        fi
        note="after $t key(s)"
        ;;
    track)
        # Finds the player's world matrix: world matrices near X,Y,Z, then
        # those of them that moved along their forward axis while KEY was
        # held (see tools/bench/gamemem.py).
        (($# >= 1)) || die "usage: track X,Y,Z [RADIUS] [KEY] [SECONDS]"
        local sock=$out/mem.sock
        [[ -S $sock ]] || die "track needs MEMORY=1 when the game is started"
        gamemem ask "$sock" scan "$1" "${2:-40}" >"$out/track-before.txt"
        press "${3:-w}" "${4:-2}"
        sleep 1
        local -a addrs
        mapfile -t addrs < <(cut -d' ' -f1 "$out/track-before.txt")
        ((${#addrs[@]})) || {
            echo "$(elapsed)s track: no world matrix near $1" >>"$out/timeline.txt"
            return 1
        }
        gamemem ask "$sock" read "${addrs[@]}" >"$out/track-after.txt"
        gamemem pick "$out/track-before.txt" "$out/track-after.txt" >"$out/track.txt"
        awk '$2 == "player" { print $1, $4, $5, $6, $7 }' "$out/track.txt" >"$out/player.addr"
        [[ -s $out/player.addr ]] || {
            echo "$(elapsed)s track: ${#addrs[@]} matrices, none moved forward" >>"$out/timeline.txt"
            return 1
        }
        note="${#addrs[@]} matrices, $(wc -l <"$out/track.txt") moved, player $(head -1 "$out/player.addr")"
        echo "$(elapsed)s $kind $* $note" >>"$out/timeline.txt"
        step pos
        return
        ;;
    find)
        # The player's world matrices at a known position, without moving:
        # loading a save puts the player at the same place every time
        # (resume.txt: 179.410,21.165,-25.438).
        (($# >= 1)) || die "usage: find X,Y,Z [TOLERANCE]"
        [[ -S $out/mem.sock ]] || die "find needs MEMORY=1 when the game is started"
        local found
        found=$(gamemem find "$out/mem.sock" "$1" "${2:-0.05}" "$out/player.addr")
        ((found)) || {
            echo "$(elapsed)s find: no upright world matrix at $1" >>"$out/timeline.txt"
            return 1
        }
        note="$found matrices"
        ;;
    walk)
        # Holds KEY until the player has moved METRES, stops making progress
        # (blocked: less than 0.15 m in 6 readings) or TIMEOUT seconds pass. The
        # pace of the game follows its frame rate, so a fixed `hold` covers
        # a different distance on every run.
        (($# >= 2)) || die "usage: walk KEY METRES [TIMEOUT]"
        step pos >/dev/null || return 1
        local x0 z0 t0 why=timeout
        read -r x0 _ z0 _ <"$out/pos.txt"
        t0=$(elapsed)
        echo "# walk $*: time distance x y z dx dz yaw" >>"$out/walk.txt"
        local -a trail=()
        xdotool keydown "$1" || true
        while :; do
            sleep 1
            gamemem where "$out/mem.sock" "$out/player.addr" >"$out/pos.txt" 2>/dev/null || break
            local x z t d
            read -r x _ z _ <"$out/pos.txt"
            t=$(elapsed)
            d=$(awk -v a="$x" -v b="$z" -v c="$x0" -v e="$z0" 'BEGIN { printf "%.2f", sqrt((a - c) ^ 2 + (b - e) ^ 2) }')
            trail+=("$t $d")
            echo "$t $d $(<"$out/pos.txt")" >>"$out/walk.txt"
            if awk -v d="$d" -v m="$2" 'BEGIN { exit !(d >= m) }'; then
                why=reached
                break
            fi
            # Progress over the last 6 readings, once 15 s have passed: the
            # player turns first, and under load the first metre may take
            # 10 s.
            if ((${#trail[@]} > 6)) && awk -v t="$t" -v t0="$t0" -v d="$d" -v old="${trail[-7]#* }" \
                'BEGIN { exit !(t - t0 >= 15 && d - old < 0.15) }'; then
                why=blocked
                break
            fi
            awk -v t="$t" -v t0="$t0" -v m="${3:-40}" 'BEGIN { exit !(t - t0 >= m) }' && break
        done
        xdotool keyup "$1" || true
        sleep 0.5
        step pos >/dev/null || return 1
        local dist
        dist=$(awk -v c="$x0" -v e="$z0" '{ printf "%.2f", sqrt(($1 - c) ^ 2 + ($3 - e) ^ 2) }' "$out/pos.txt")
        note="$why: $dist m from $x0,$z0 in $(awk -v a="$(elapsed)" -v b="$t0" 'BEGIN { printf "%.0f", a - b }') s, now $(<"$out/pos.txt")"
        ;;
    pos)
        # Position and facing of the player found by `find` or `track`:
        # OUT/pos.txt "x y z dx dz yaw" (dx, dz: unit facing on the ground,
        # yaw in degrees from +z towards +x).
        [[ -s $out/player.addr && -S $out/mem.sock ]] || die "pos needs \`find\` or \`track\` first"
        rm -f -- "$out/pos.txt"
        gamemem where "$out/mem.sock" "$out/player.addr" >"$out/pos.txt" || true
        [[ -s $out/pos.txt ]] || {
            echo "$(elapsed)s pos: the player's matrices are gone (find or track again)" >>"$out/timeline.txt"
            return 1
        }
        note="$(<"$out/pos.txt")"
        ;;
    *) die "unknown step '$kind'" ;;
    esac
    echo "$(elapsed)s $kind $* $note" >>"$out/timeline.txt"
}

gamemem() {
    python3 "$here/bench/gamemem.py" "$@" </dev/null
}

# Reference images for `await`: parts of the screen, as raw grey pixels.
refs_dir() {
    echo "${REFS:-$HOME/.local/share/doctor_toolkit/bench-refs}"
}

# Grey pixels of the area X,Y,W,H of the screen, or of PNG when given.
area_pixels() {
    local x y w h
    IFS=, read -r x y w h <<<"$1"
    if [[ -n ${2:-} ]]; then
        ffmpeg -loglevel error -i "$2" -vf "crop=$w:$h:$x:$y" -f rawvideo -pix_fmt gray - </dev/null
    else
        ffmpeg -loglevel error -f x11grab -video_size "${w}x$h" -i "$display+$x,$y" \
            -frames:v 1 -f rawvideo -pix_fmt gray - </dev/null
    fi
}

# Saves the area X,Y,W,H of the screen (or of PNG) as reference NAME.
save_ref() {
    local dir
    dir=$(refs_dir)
    mkdir -p "$dir"
    area_pixels "$2" "${3:-}" >"$dir/$1.gray"
    echo "$2" >"$dir/$1.area"
    [[ -s $dir/$1.gray ]] || die "could not read the area $2"
}

# Waits until the area of reference NAME looks like it: mean absolute grey
# difference at most TOLERANCE (default 12 of 255). `!NAME` waits until it
# no longer does. Fails after TIMEOUT seconds (default 600) or when the game
# exits.
await_ref() {
    local name=$1 timeout=${2:-600} tolerance=${3:-12} dir negate=0
    if [[ $name == !* ]]; then
        negate=1
        name=${name#!}
    fi
    dir=$(refs_dir)
    [[ -f $dir/$name.gray && -f $dir/$name.area ]] || die "no reference $name in $dir"
    local area deadline current
    area=$(<"$dir/$name.area")
    deadline=$(awk -v e="$(elapsed)" -v t="$timeout" 'BEGIN { printf "%.1f", e + t }')
    current=$out/.await.gray
    while :; do
        area_pixels "$area" >"$current" || true
        if python3 -c '
import sys
a = open(sys.argv[1], "rb").read()
b = open(sys.argv[2], "rb").read()
diff = sum(abs(x - y) for x, y in zip(a, b)) / len(a) if a and len(a) == len(b) else 255
match = diff <= float(sys.argv[3])
sys.exit(0 if match != (sys.argv[4] == "1") else 1)
' "$dir/$name.gray" "$current" "$tolerance" "$negate"; then
            rm -f -- "$current"
            return 0
        fi
        kill -0 "$game_pid" 2>/dev/null || return 1
        # A crash leaves the game running under the Wine debugger window.
        if grep -q "Unhandled .* starting debugger" "$out/wine.log" 2>/dev/null; then
            echo "$(elapsed)s the game crashed: $(grep -o "Unhandled [^,]*" "$out/wine.log" | head -1)" \
                >>"$out/timeline.txt"
            return 1
        fi
        awk -v e="$(elapsed)" -v d="$deadline" 'BEGIN { exit !(e >= d) }' && return 1
        sleep 1
    done
}

# Saves an area of a screenshot as a reference for `await`.
cmd_ref() {
    (($# == 3)) || die "usage: ref PNG NAME X,Y,W,H"
    save_ref "$2" "$3" "$1"
    echo "reference $2: $3 of $1 -> $(refs_dir)"
}

finish() {
    local status=$1
    echo "status: $status" | tee -a "$out/timeline.txt"
    pgrep -fa 'godfather2.exe' >"$out/processes.txt" || true
    cleanup
    trap - EXIT
    # Links out of the prefix made during the run (see bench_home).
    local replaced
    replaced=$(sanitize_prefix "$prefix")
    if [[ -n $replaced ]]; then
        echo "WARNING: $replaced" | tee -a "$out/timeline.txt"
    fi
    echo "output: $out"
}

cmd_run() {
    (($# == 2)) || die "usage: run SHADOW OUT"
    setup "$@"
    local duration=${DURATION:-90}
    local shots=${SHOTS:-}
    if [[ -z $shots ]]; then
        shots=$(seq 10 10 "$duration" | tr '\n' ' ')
    fi
    trap cleanup EXIT
    trap 'exit 130' INT TERM
    launch

    local -a events=()
    local t
    for t in $shots; do events+=("$t shot"); done
    for t in ${CLICKS:-}; do events+=("${t%%:*} click ${t#*:}"); done
    for t in ${KEYS:-}; do events+=("${t%%:*} key ${t#*:}"); done
    for t in ${MOVES:-}; do events+=("${t%%:*} move ${t#*:}"); done
    mapfile -t events < <(printf '%s\n' "${events[@]+"${events[@]}"}" | sort -n -k1,1 -s | sed '/^$/d')

    local status=running at kind arg shot_count=0
    for event in "${events[@]+"${events[@]}"}" "$duration end"; do
        read -r at kind arg <<<"$event"
        step until "$at" >/dev/null
        if ! kill -0 "$game_pid" 2>/dev/null; then
            status="exited after $(elapsed)s"
            break
        fi
        case $kind in
        shot)
            shot_count=$((shot_count + 1))
            step shot "shot_$(printf %02d "$shot_count")_$(printf %03d "${at%.*}")s"
            ;;
        click | key | move) step "$kind" "$arg" ;;
        end) ;;
        esac
    done
    finish "$status"
}

# Runs the steps of a scenario file: one step per line, `#` comments.
cmd_play() {
    (($# == 3)) || die "usage: play SHADOW OUT SCENARIO"
    local scenario
    scenario=$(realpath "$3")
    [[ -f $scenario ]] || die "no scenario $scenario"
    setup "$1" "$2"
    cp -- "$scenario" "$out/scenario.txt"
    trap cleanup EXIT
    trap 'exit 130' INT TERM
    launch
    local status=finished line
    local -a words
    while IFS= read -r line || [[ -n $line ]]; do
        line=${line%%#*}
        read -r -a words <<<"$line"
        ((${#words[@]})) || continue
        if ! kill -0 "$game_pid" 2>/dev/null; then
            status="exited after $(elapsed)s"
            break
        fi
        if ! step "${words[@]}"; then
            status="failed at '${words[*]}' after $(elapsed)s"
            break
        fi
    done <"$scenario"
    if [[ $status == finished ]] && ! kill -0 "$game_pid" 2>/dev/null; then
        status="exited after $(elapsed)s"
    fi
    finish "$status"
}

# A game left running for `do` steps; stopped by `stop`, by the game
# exiting, or after SESSION_MAX seconds.
cmd_start() {
    (($# == 2)) || die "usage: start SHADOW OUT"
    local o
    o=$(realpath -m "$2")
    mkdir -p "$o"
    [[ ! -e $o/session.env ]] || die "a session is already running in $o (\`stop $o\`)"
    rm -f -- "$o/.stop"
    setsid "$0" _session "$1" "$o" >"$o/session.log" 2>&1 </dev/null &
    for _ in $(seq 600); do
        if [[ -e $o/session.env ]]; then
            cat "$o/session.env"
            return
        fi
        kill -0 $! 2>/dev/null || die "the session did not start, see $o/session.log"
        sleep 0.2
    done
    die "the session did not start in 120 s, see $o/session.log"
}

cmd__session() {
    setup "$@"
    trap 'rm -f -- "$out/session.env"; cleanup' EXIT
    trap 'exit 130' INT TERM HUP
    launch
    printf 'display=%s\nstart=%s\ngame_pid=%s\nsize=%s\nprefix=%q\n' \
        "$display" "$start" "$game_pid" "$size" "$prefix" >"$out/session.env.tmp"
    mv -- "$out/session.env.tmp" "$out/session.env"
    local status
    while :; do
        if ! kill -0 "$game_pid" 2>/dev/null; then
            status="exited after $(elapsed)s"
            break
        fi
        if [[ -e $out/.stop ]]; then
            status="stopped after $(elapsed)s"
            break
        fi
        if awk -v e="$(elapsed)" -v m="${SESSION_MAX:-3600}" 'BEGIN { exit !(e >= m) }'; then
            status="SESSION_MAX reached after $(elapsed)s"
            break
        fi
        sleep 1
    done
    finish "$status"
    rm -f -- "$out/session.env" "$out/.stop"
}

# Steps against a running session, separated by `--`.
cmd_do() {
    (($# >= 2)) || die "usage: do OUT STEP [ARG...] [-- STEP [ARG...]]..."
    out=$(realpath "$1")
    shift
    [[ -f $out/session.env ]] || die "no session is running in $out"
    # shellcheck source=/dev/null
    source "$out/session.env"
    export DISPLAY=$display
    local -a words=()
    local arg
    for arg in "$@" --; do
        if [[ $arg == -- ]]; then
            if ((${#words[@]})); then
                kill -0 "$game_pid" 2>/dev/null || die "the game has exited"
                step "${words[@]}" || die "step '${words[*]}' failed"
            fi
            words=()
        else
            words+=("$arg")
        fi
    done
}

cmd_stop() {
    (($# == 1)) || die "usage: stop OUT"
    local o
    o=$(realpath "$1")
    [[ -f $o/session.env ]] || die "no session is running in $o"
    : >"$o/.stop"
    for _ in $(seq 60); do
        [[ -e $o/session.env ]] || {
            tail -2 "$o/timeline.txt"
            return
        }
        sleep 0.5
    done
    die "the session in $o did not stop; see $o/session.log"
}

# The point N metres ahead of the player (OUT/pos.txt of `pos`), DY metres
# up and SIDE metres to the right: "x,y,z" for `level add --position`.
cmd_ahead() {
    (($# >= 2 && $# <= 4)) || die "usage: ahead OUT N [DY] [SIDE]"
    [[ -s $1/pos.txt ]] || die "no $1/pos.txt (step \`pos\`)"
    awk -v n="$2" -v up="${3:-0}" -v side="${4:-0}" '{
        # Right of the facing (dx, dz) with y up: (-dz, dx) in x, z.
        printf "%.3f,%.3f,%.3f\n", $1 + n * $4 - side * $5, $2 + up, $3 + n * $5 + side * $4
    }' "$1/pos.txt"
}

# Copies the profiles and saves of the bench prefix to DEST, for SAVES=DEST.
cmd_saves() {
    (($# == 1)) || die "usage: saves DEST"
    prefix=${BENCH_PREFIX:-$HOME/.local/share/doctor_toolkit/wine-gf2-bench}
    local dest documents local_dir
    dest=$(realpath -m "$1")
    { read -r documents && read -r local_dir; } < <(save_dirs)
    [[ -d "$documents/EA Games" ]] || die "the bench prefix has no profiles yet"
    rm -rf -- "$dest"
    mkdir -p "$dest/Documents" "$dest/Local"
    cp -a -- "$documents/EA Games" "$dest/Documents/"
    if [[ -d "$local_dir/EA Games" ]]; then
        cp -a -- "$local_dir/EA Games" "$dest/Local/"
    fi
    # The profile folder is a link to the documents directory of the bench
    # home (see bench_home): copy what it holds instead, so that loading the
    # saves makes a real folder and Wine creates no link.
    local link target real_prefix
    real_prefix=$(realpath "$prefix")
    while IFS= read -r -d '' link; do
        target=$(realpath -m -- "$link")
        rm -- "$link"
        case $target/ in
        "$real_prefix"/*)
            mkdir -- "$link"
            cp -a -- "$target/." "$link/"
            ;;
        *) echo "dropped link $link to $target outside the prefix" ;;
        esac
    done < <(find "$dest" -type l -print0)
    find "$dest" -type f -printf '%P %s\n'
}

case ${1:-} in
shadow | verify | run | play | start | do | stop | saves | ref | ahead)
    command=$1
    shift
    "cmd_$command" "$@"
    ;;
_session)
    shift
    cmd__session "$@"
    ;;
*)
    sed -n '2,/^set -euo/p' "$0" | sed '$d; s/^# \{0,1\}//'
    exit 2
    ;;
esac

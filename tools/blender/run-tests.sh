#!/usr/bin/env bash
# Round trip of a game model through headless Blender (tools/blender/tests/roundtrip.py).
#
#   tools/blender/run-tests.sh --game "/path/to/The Godfather II" --wheel dist/doctor_toolkit-*.whl
#
# Blender runs in background mode (-b) with factory settings and every user
# folder (configuration, scripts, extensions) in a temporary folder, so the
# user's Blender settings and add-ons are not read or changed. The wheel is
# unpacked into that folder and added to sys.path; nothing is installed into
# Blender. The game folder is only read: the test works on copies.
#
# Options: --blender PATH (default: $BLENDER or `blender`), --wheel FILE or
# --module-path DIR (folder with the doctor_toolkit module), --work DIR (kept
# afterwards; default: a temporary folder that is removed), --extension ZIP
# (also install that extension package into the temporary Blender folder and
# drive the panel's operators on the copied game, tests/extension.py, searching
# --query TEXT, default dmg0_model), --cli PATH (also run `PATH edit … --format
# glb` with headless Blender as the editor, tests/link_e2e.py), and the options
# of roundtrip.py (--stream, --model, --extra).
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
blender="${BLENDER:-blender}"
wheel=""
module_path=""
work=""
game=""
extension=""
query="dmg0_model"
cli=""
stream="mp_rooftops/mp_rooftops"
model=""
pass=()
while (($#)); do
    case "$1" in
        --blender) blender="$2"; shift 2 ;;
        --wheel) wheel="$2"; shift 2 ;;
        --module-path) module_path="$2"; shift 2 ;;
        --work) work="$2"; shift 2 ;;
        --game) game="$2"; shift 2 ;;
        --extension) extension="$2"; shift 2 ;;
        --query) query="$2"; shift 2 ;;
        --cli) cli="$2"; shift 2 ;;
        --stream) stream="$2"; pass+=("$1" "$2"); shift 2 ;;
        --model) model="$2"; pass+=("$1" "$2"); shift 2 ;;
        *) pass+=("$1"); shift ;;
    esac
done
if [[ -z "$game" ]]; then
    game="${EARS_GF2_DIR:-}"
fi
if [[ -z "$game" ]]; then
    echo "usage: $0 --game GAME_DIR (--wheel FILE | --module-path DIR) [--blender PATH]" >&2
    exit 2
fi

temp="$(mktemp -d "${TMPDIR:-/tmp}/doctor-toolkit-blender.XXXXXX")"
keep_work=1
if [[ -z "$work" ]]; then
    work="$temp/work"
    keep_work=0
fi
cleanup() { rm -rf -- "$temp"; }
trap cleanup EXIT

if [[ -n "$wheel" ]]; then
    module_path="$temp/site"
    mkdir -p "$module_path"
    python3 -m zipfile -e "$wheel" "$module_path"
fi
if [[ -z "$module_path" ]]; then
    echo "give --wheel or --module-path" >&2
    exit 2
fi

user="$temp/blender-user"
mkdir -p "$user"/{config,scripts,datafiles,extensions}
export BLENDER_USER_RESOURCES="$user"
export BLENDER_USER_CONFIG="$user/config"
export BLENDER_USER_SCRIPTS="$user/scripts"
export BLENDER_USER_DATAFILES="$user/datafiles"
export BLENDER_USER_EXTENSIONS="$user/extensions"
export DOCTOR_TOOLKIT_CACHE_DIR="$temp/cache"
export XDG_STATE_HOME="$temp/state"

status=0
"$blender" -b --factory-startup --python-exit-code 1 \
    --python "$here/tests/roundtrip.py" -- \
    --game "$game" --work "$work" --module-path "$module_path" "${pass[@]}" </dev/null || status=$?
if ((status == 0)) && [[ -n "$extension" ]]; then
    "$blender" --command extension install-file -r user_default -e "$extension" </dev/null
    "$blender" -b --python-exit-code 1 --python "$here/tests/extension.py" -- \
        --game "$work/game" --query "$query" </dev/null || status=$?
fi
if ((status == 0)) && [[ -n "$cli" ]]; then
    python3 "$here/tests/link_e2e.py" --cli "$cli" --blender "$blender" --game "$work/game" \
        --module-path "$module_path" --stream "$stream" --model "$model" --work "$work" ||
        status=$?
fi
if ((keep_work)); then
    echo "work files kept in $work"
fi
exit "$status"

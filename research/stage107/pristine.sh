#!/usr/bin/env bash
# pristine.sh: a session on a shadow without any mod, for comparing the frame.
here=$(dirname "$(realpath "$0")")
. "$here/env.sh"
bash "$ROOT/research/stage100/mkshadow.sh" pristine
if [[ -n ${LOADER:-} ]]; then cp "$LOADER" "$S/pristine/dinput8.dll"; fi
mkdir -p "$S/pristine/scripts"
PLAIN=1 SHADOW_NAME=pristine bash "$here/run.sh" pristine 2>&1 |
    grep -E 'redirected|lua|cf7867b7|in all|exception|state of|session exit' | cut -c1-200

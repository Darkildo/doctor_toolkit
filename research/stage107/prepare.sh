#!/usr/bin/env bash
# prepare.sh: the shadows, the base mod (written into the shadow) and the
# project of the stage 107 check. Nothing is written into the game folder.
set -euo pipefail
here=$(dirname "$(realpath "$0")")
. "$here/env.sh"
STREAM=gf2/cuba/local/yuri/high/terrain3

rm -rf "$S/base-ws" "$S/proj" "$S/base.zip" "$S/config" "$S/cache"
for name in shadow expected; do
    bash "$ROOT/research/stage100/mkshadow.sh" "$name"
    # A game without an ASI loader of its own: the shadow gets one.
    if [[ -n ${LOADER:-} ]]; then cp "$LOADER" "$S/$name/dinput8.dll"; fi
    mkdir -p "$S/$name/scripts"
done

# The project: a folder with its own workspace (what File -> New project makes).
mkdir -p "$S/proj/.doctor"
cat >"$S/proj/project.toml" <<EOF
format = 1

[project]
name = "Bench Project"
id = "bench-project"
version = "0.7"
author = "bench"

[game]
profile = "The Godfather II (PC)"

[build]
workspace = "workspace"
EOF
echo "game = \"$S/shadow\"" >"$S/proj/.doctor/local.toml"

python3 "$here/setup.py" "$S/shadow" "$S/base-ws" "$S/proj" | tee "$S/setup.jsonl"

"$T" mods build "$S/shadow" --name "Bench Base" --id bench-base --mod-version 1.3 \
    --author bench --dir "$S/base-ws" -o "$S/base.zip" | tail -2
for name in shadow expected; do
    "$T" mods install "$S/$name" "$S/base.zip" | head -3
    "$T" mods apply "$S/$name" | tail -2
done
# The reference: the project installed as a mod and written, in its own shadow.
"$T" project install "$S/proj" --game "$S/expected"
"$T" mods apply "$S/expected" | tail -3
"$T" mods list "$S/expected"
echo "--- the shadow the game runs from:"
"$T" mods list "$S/shadow"
"$T" workspace status "$S/shadow" --dir "$S/proj/workspace" | tail -5

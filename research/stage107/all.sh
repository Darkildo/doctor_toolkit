#!/usr/bin/env bash
# all.sh: prepare, then the sessions — project staged, base only, both written.
here=$(dirname "$(realpath "$0")")
bash "$here/prepare.sh" 2>&1 | grep -E '"texture"|"lua"|enabled' | cut -c1-120
for run in "project" "base PLAIN=1" "written PLAIN=1 SHADOW_NAME=expected" "memory MEM=1"; do
    set -- $run
    name=$1
    shift
    extra=()
    [[ $name == memory ]] && extra=(--resources-in-memory)
    echo "=== $name"
    env "$@" bash "$here/run.sh" "$name" "${extra[@]}" 2>&1 |
        grep -E 'redirected|str ->|lua|cf7867b7|in all|other size|exception|state of|session exit|DIFFERS|^[<>]' | cut -c1-200
done

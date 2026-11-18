#!/usr/bin/env bash
# state.sh DIR: the state of a game folder, one line per entry — SHA-256 of
# every file; target, size and time of change of every link. The toolkit's
# folder is included (the registry of the mod manager, its record of what is
# written, the backups); the bench's own folder and the files a run of the
# game writes by itself are not.
set -euo pipefail
cd "$1"
find . \( -path ./.game-bench -o -path ./_doctor_toolkit/live -o -path ./_doctor_toolkit/crashes \) -prune -o -type f -print0 |
    sort -z | xargs -0 sha256sum
find . -path ./.game-bench -prune -o -type l -printf '%p -> %l %s %T@\n' | sort

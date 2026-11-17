#!/usr/bin/env bash
# compare_streams.sh A.str B.str: whether two stream files hold the same
# resources — the same headers in the same order (`list` without the stored
# size) and the same decompressed data (`extract`). Prints "same file" when
# the files are equal byte for byte, "same resources" when only the storage
# differs, and fails otherwise.
set -euo pipefail
T=${T:-doctor-toolkit}
a=$1
b=$2
if cmp -s "$a" "$b"; then
    echo "same file ($(stat -c %s "$a") bytes): $a"
    exit 0
fi
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
rows() { "$T" list "$1" | awk 'NR > 1 { $NF = ""; if ($(NF-1) ~ /^[0-9]+$/) $(NF-1) = ""; print }' | sed 's/ refpack//; s/ *$//'; }
# index, type, policy, alignment, ids, size: everything but how it is stored.
"$T" list "$a" | awk 'NR > 1 { n = ($NF == "refpack") ? NF - 2 : NF - 1; line = ""; for (i = 1; i <= n; i++) line = line " " $i; print line }' >"$tmp/a.txt"
"$T" list "$b" | awk 'NR > 1 { n = ($NF == "refpack") ? NF - 2 : NF - 1; line = ""; for (i = 1; i <= n; i++) line = line " " $i; print line }' >"$tmp/b.txt"
if ! cmp -s "$tmp/a.txt" "$tmp/b.txt"; then
    echo "DIFFERENT headers: $a $b"
    diff "$tmp/a.txt" "$tmp/b.txt" | head -20
    exit 1
fi
"$T" extract "$a" -o "$tmp/a" >/dev/null
"$T" extract "$b" -o "$tmp/b" >/dev/null
if diff -rq "$tmp/a" "$tmp/b" >"$tmp/diff.txt"; then
    echo "same resources ($(wc -l <"$tmp/a.txt"), sizes $(stat -c %s "$a") and $(stat -c %s "$b") bytes, storage differs): $a"
else
    echo "DIFFERENT data: $a $b"
    head -20 "$tmp/diff.txt"
    exit 1
fi

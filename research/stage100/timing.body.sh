# Body of a timing session (sourced by session.sh): the load of the level,
# then a tour of far points of the city; at each the seconds until the list
# of loaded zone streams stops changing, and the reads of the game's file device
# in between (the module's profile: reads, bytes and time of the read at
# 0x00ABEC29 — with resources in memory that time includes the module).
#   TOUR  "NAME:X,Y,Z ..." (default: three points of Havana, twice)
TOUR=${TOUR:-"west:-512,14,145 mid:-72.5,12,86.1 spawn:283.9,0.5,371.8 west2:-512,14,145 mid2:-72.5,12,86.1 spawn2:283.9,0.5,371.8"}
io() {
    # reads, bytes and milliseconds of the file device's read since the
    # reset; megabytes committed in the process and held by textures
    python3 -c '
import json, sys
p = json.load(open(sys.argv[1]))
site = [s for s in p.get("io_sites", []) if s.get("site") == "0x00abec29"]
s = site[0] if site else {"reads": 0, "bytes": 0, "total_ms": 0}
m = p.get("memory") or {}
t = p.get("textures") or {}
print(s["reads"], s["bytes"], round(s["total_ms"], 2), m.get("committed_mb", 0), t.get("mb", 0))
' "$1"
}
settle 4 >/dev/null
L profile --memory --reset --json >"$OUT/profile-load.json" 2>>"$OUT/run.log"
echo "load $(cat "$OUT/load_times.txt") $(io "$OUT/profile-load.json") $(L streams 2>/dev/null | grep -ci '\\local\\')" | tee -a "$OUT/timing.txt"
for point in $TOUR; do
    name=${point%%:*}
    xyz=${point#*:}
    IFS=, read -r x y z <<<"$xyz"
    t0=$(now)
    tp "$x" "$y" "$z"
    t=$(settle 4)
    L profile --memory --reset --json >"$OUT/profile-$name.json" 2>>"$OUT/run.log"
    echo "$name $t $(io "$OUT/profile-$name.json") $(L streams 2>/dev/null | grep -ci '\\local\\')" | tee -a "$OUT/timing.txt"
done
L resources --json >"$OUT/resources.json" 2>>"$OUT/run.log"
L resources >"$OUT/resources.txt" 2>>"$OUT/run.log"
L status | tee -a "$OUT/run.log"
shot end

#!/usr/bin/env bash
# A new made man for the player's family (stage 82).
#
#   examples/crew/new-crew-member.sh GAME_COPY WORKSPACE [MOD.zip]
#
# GAME_COPY is a copy of the game (or a shadow made by `tools/game-bench.sh
# shadow`); nothing is written into it: every change is recorded in WORKSPACE.
# With MOD.zip the workspace is built into a mod for the mod manager. Give the
# workspace to the game with `doctor-toolkit live install GAME_COPY --apply
# --workspace-dir WORKSPACE`, or write it with `workspace apply`.
#
# What a made man is in the game's data (docs/kb/systems/crew.md):
#
#   CommonNPC template (gf2/globals_global)      SimNpc (gf2/globals_global)
#     NPC.2  designer name `NPC_CRW_*`             SimNpc.1  -> the template
#     NPC.18 hash of the text `name_<NPC.2>`       SimNpc.0  flags: weapon licence
#     PartedAnimated.0/1/2 the look                          (bit 3 Advanced, bit 4 Expert)
#     NPCCrewComponent.0 specialties (0x04..0x80)
#     Sentient.2 family `corleone`
#
#   He joins the family in one of two ways:
#     AddPlayerFamilyMember (message, SimNpc, rank, specialties): at once;
#     SpecificNpcRequester + SpawnPoint: he stands in the world, the player
#       talks to him and hires him (as the game places its own recruits).
#
# The script makes all of it from copies of the game's own objects: the
# texts of the name and of the biography, a look of his own (a new item of an
# appearance preset of the `player` parts database), the template, the SimNpc,
# an AddPlayerFamilyMember that hears `<prefix>_join`, and, with CREW_PLACE,
# a recruit standing at that place of New York. What was checked in the game
# and what was not: docs/analysis/game-bench.md §44. One thing does not work:
# his portrait in the menus is the default head
# (docs/kb/questions/crew-portrait-of-new-member.md).
set -euo pipefail
game=${1:?game copy}
ws=${2:?workspace directory}
mod=${3:-}
t=${DOCTOR_TOOLKIT:-doctor-toolkit}

# --- what to change for a made man of your own -----------------------------
prefix=${CREW_PREFIX:-dcrew}            # every message and text of the example starts with it
name=${CREW_NAME:-Sal Dottore}
name_ru=${CREW_NAME_RU:-Сэл Доктор}
bio=${CREW_BIO:-Sal patched up half of Queens before he joined the family. A made man of the doctor_toolkit example.}
bio_ru=${CREW_BIO_RU:-Сэл подлатал пол-Квинса, прежде чем пришёл в семью. Человек чести из примера doctor_toolkit.}
base=${CREW_BASE:-NPC_CRW_0006}         # the recruit of the game he is a copy of (NPC.2 of the template)
specialties=${CREW_SPECIALTIES:-medic,arsonist}   # of demolition arsonist safe_cracker engineer medic bruiser
rank=${CREW_RANK:-3}                    # for <prefix>_join: 3 soldier, 4 capo, 5 underboss
licence=${CREW_LICENCE:-2}              # weapon licence he comes with: 1 Basic, 2 Advanced, 3 Expert
health=${CREW_HEALTH:-3500}
# The look: an item `look` is added to the appearance preset of the base
# recruit; its materials are those of the items `look_a` and `look_b` of the
# same preset, taken in turn (a mix no recruit of the game wears).
look=${CREW_LOOK:-doctor}
look_a=${CREW_LOOK_A:-havana}
look_b=${CREW_LOOK_B:-miami}
# A recruit standing in New York: X,Y,Z (Y is up); empty: no recruit, only
# the message. He appears `spawn_delay` seconds after New York loads.
place=${CREW_PLACE-"-508,6.63,-246"}
spawn_delay=${CREW_SPAWN_DELAY:-10}
# Seconds after the load of the game's globals when <prefix>_join is sent by
# itself (for a look at him from a quick start; this way was not checked in
# the game). Empty: nobody sends it; send it from a mission (`MultiManager`,
# `Checkpoint`) or with `live send`.
auto_join=${CREW_AUTO_JOIN:-}
# ---------------------------------------------------------------------------

globals=gf2/globals_global
level=gf2/queens
recruits=gf2/queens/local/heidi/high/terrain04   # a recruit of the game: requester, spawn point, trigger
characters=gf2/globalfolder/characters/allcharacters

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

simgroup_of() { "$t" list "$game/$1.str" | awk '$2 == "simgroup" { print $1; exit }'; }
globals_group=$(simgroup_of "$globals")
level_group=$(simgroup_of "$level")
recruits_group=$(simgroup_of "$recruits")

# add STREAM GROUP TEMPLATE_STREAM TEMPLATE_GROUP TEMPLATE_OBJECT [--position=X,Y,Z] -> "index guid"
add() {
    local stream=$1 group=$2 from=$3 from_group=$4 object=$5 out
    shift 5
    out=$("$t" level add "$game" "$stream" "$group" "$object" \
        --template-stream "$from" --template-resource "$from_group" --force "$@" --workspace-dir "$ws")
    sed -n 's/^added object \([0-9]*\) (\([0-9a-f]*\)).*/\1 \2/p' <<<"$out"
}
# set STREAM GROUP OBJECT Class.attr=value ...
set_attrs() {
    local stream=$1 group=$2 object=$3 args=()
    shift 3
    for a in "$@"; do args+=(--set "$a"); done
    "$t" simgroup-set "$game" "$stream" "$group" "$object" "${args[@]}" --workspace-dir "$ws" >/dev/null
}

# 1. The objects of the game the new ones are copies of, found by what they
#    are and not by their numbers (mods move the numbers).
"$t" logic-export "$game" -o "$tmp" --type simgroup --stream "$globals" >/dev/null
"$t" logic-export "$game" -o "$tmp" --type simgroup --stream "$recruits" >/dev/null
eval "$(python3 - "$tmp" "$globals" "$recruits" "$base" "$specialties" "$licence" "$prefix" <<'EOF'
import glob, json, sys
tmp, globals_, recruits, base, specialties, licence, prefix = sys.argv[1:]
BITS = {"demolition": 0x04, "arsonist": 0x08, "safe_cracker": 0x10, "engineer": 0x20, "medic": 0x40, "bruiser": 0x80}
HIRED = {0x04: "iMsgRecruitedDemolitions", 0x08: "iMsgRecruitedArsonist", 0x10: "iMsgRecruitedSafecracker",
         0x20: "iMsgRecruitedEngineer", 0x40: "iMsgRecruitedMedic", 0x80: "iMsgRecruitedBruiser"}

def objects(stream):
    (path,) = glob.glob("%s/%s/*_simgroup_*.json" % (tmp, stream))
    return json.load(open(path))["objects"]

def attrs(o):
    return {(p.get("class_name") or p["class"], a["index"]): a["value"] for p in o["packets"] for a in p["attrs"]}

def sdbm(text, h=0):
    for c in text.lower().encode():
        h = (c + (h << 6) + (h << 16) - h) & 0xFFFFFFFF
    return h

g = objects(globals_)
template = sim = member = auto = None
for i, o in enumerate(g):
    a = attrs(o)
    if o.get("class_name") == "CommonNPC" and a.get(("NPC", 2)) == base:
        template, template_attrs = i, a
    if o.get("class_name") == "AddPlayerFamilyMember" and member is None:
        member = i
    if o.get("class_name") == "TriggerAuto" and auto is None:
        auto = i
if template is None:
    sys.exit("no recruit %s in %s" % (base, globals_))
for i, o in enumerate(g):
    a = attrs(o)
    if o.get("class_name") == "SimNpc" and a.get(("SimNpc", 1)) == g[template]["guid"]:
        sim, sim_attrs = i, a
mask = 0
for s in specialties.split(","):
    if s not in BITS:
        sys.exit("unknown specialty %s (one of %s)" % (s, " ".join(BITS)))
    mask |= BITS[s]
word = (template_attrs.get(("NPCCrewComponent", 0), 0x101) & ~0xFC) | mask
flags = (sim_attrs.get(("SimNpc", 0), 3) & ~0x18) | {"1": 0, "2": 0x08, "3": 0x10}[licence]
r = objects(recruits)
requester = spawn = trigger = None
for i, o in enumerate(r):
    c = o.get("class_name")
    if c == "SpecificNpcRequester" and requester is None:
        requester, requester_guid = i, o["guid"]
    if c == "TriggerAuto" and trigger is None:
        trigger = i
for i, o in enumerate(r):
    if o.get("class_name") == "SpawnPoint" and attrs(o).get(("SpawnPoint", 0)) == requester_guid:
        spawn = i
out = {
    "template": template, "sim": sim, "member": member, "auto": auto,
    "look_set": template_attrs[("PartedAnimated", 0)], "look_preset": template_attrs[("PartedAnimated", 2)],
    "mask": mask, "word": word, "flags": flags, "hired": HIRED.get(mask, ""),
    "name_hash": "%08x" % sdbm("name_npc_crw_" + prefix),
    "requester": requester, "spawn": spawn, "trigger": trigger,
}
for k, v in out.items():
    if v is None:
        sys.exit("the game has no object to copy for: " + k)
    print("%s=%s" % (k, v))
EOF
)"

# 2. Texts: the name and the biography. The game's recruits call the text of
#    the name `name_<NPC.2>` (`name_npc_crw_0006`); the template holds its hash
#    (NPC.18). The biography is found by the name: its text is called
#    <name of the name's text>_background (0x009263B0).
"$t" text-add "$game" "name_npc_crw_${prefix}" "$name" --lang ru="$name_ru" --workspace-dir "$ws" >/dev/null
"$t" text-add "$game" "name_npc_crw_${prefix}_background" "$bio" --lang ru="$bio_ru" --workspace-dir "$ws" >/dev/null

# 3. The look: a new item of the recruit's appearance preset.
"$t" logic-export "$game" -o "$tmp" --type partsdb --stream "$characters" >/dev/null
player_db=$(grep -l '"name": "player"' "$tmp/$characters"/*_partsdb_*.json | head -1)
python3 - "$player_db" "$look_set" "$look_preset" "$look" "$look_a" "$look_b" <<'EOF'
import copy, json, sys
path, set_name, preset_name, look, look_a, look_b = sys.argv[1:]
db = json.load(open(path))
if db.get("name") != "player":
    sys.exit("not the parts database `player`: " + path)
(pi, preset), = [(i, p) for i, p in enumerate(db["presets"]) if p["name"] == preset_name]
items = {it["name"]: it for it in preset["items"]}
if look not in items:
    a, b = items[look_a], items[look_b]
    new = copy.deepcopy(a)
    new["name"] = look
    other = {s["from"]: s["to"] for s in b["swaps"]}
    for n, s in enumerate(new["swaps"]):
        if n % 2 and s["from"] in other:
            s["to"] = other[s["from"]]
    preset["items"].append(new)
    # Every set that uses the preset gets the item: the recruit's own set (the
    # man in the world) and `mobface` (his portraits: the game redresses a
    # `mobface` character by finding the same item there, 0x009C15E0; without
    # it the portrait shows the default head).
    used = False
    for s in db["sets"]:
        for use in s.get("presets", []):
            if use["preset"] == pi:
                use["items"].append(len(preset["items"]) - 1)
                used = used or s["name"] == set_name
    if not used:
        sys.exit("the set %s does not use the preset %s" % (set_name, preset_name))
    json.dump(db, open(path, "w"), indent=1)
EOF
"$t" logic-import "$game" "$player_db" --workspace-dir "$ws" >/dev/null

# 4. The template and its SimNpc.
read -r npc npc_guid < <(add "$globals" "$globals_group" "$globals" "$globals_group" "$template")
set_attrs "$globals" "$globals_group" "$npc" "NPC.2=NPC_CRW_${prefix^^}" "NPC.18=$name_hash" \
    "NPC.31=$hired" "PartedAnimated.1=$look" "NPCCrewComponent.0=$word" \
    "StandardDamageComponent.CMD_SetInitialHealth=$health"
read -r simnpc sim_guid < <(add "$globals" "$globals_group" "$globals" "$globals_group" "$sim")
set_attrs "$globals" "$globals_group" "$simnpc" "SimNpc.1=$npc_guid" "SimNpc.0=$flags"

# 5. <prefix>_join: he is a member of the family at once (the place of his
#    rank is opened if the story has not opened it yet).
read -r join _ < <(add "$globals" "$globals_group" "$globals" "$globals_group" "$member")
set_attrs "$globals" "$globals_group" "$join" "AddPlayerFamilyMember.0=${prefix}_join" \
    "AddPlayerFamilyMember.1=$sim_guid" "AddPlayerFamilyMember.2=$rank" "AddPlayerFamilyMember.3=$mask"
if [[ -n $auto_join ]]; then
    read -r auto_object _ < <(add "$globals" "$globals_group" "$globals" "$globals_group" "$auto")
    set_attrs "$globals" "$globals_group" "$auto_object" "TriggerAuto.0=${prefix}_join" "TriggerAuto.1=$auto_join"
fi

# 6. A recruit in the world: the trigger wakes the requester, the requester
#    asks for this SimNpc at its spawn point.
if [[ -n $place ]]; then
    read -r req req_guid < <(add "$level" "$level_group" "$recruits" "$recruits_group" "$requester")
    set_attrs "$level" "$level_group" "$req" "SpecificNpcRequester.0=${prefix}_spawn" "SpecificNpcRequester.1=$sim_guid"
    read -r point _ < <(add "$level" "$level_group" "$recruits" "$recruits_group" "$spawn" --position="$place")
    set_attrs "$level" "$level_group" "$point" "SpawnPoint.0=$req_guid" "SpawnPoint.6=${prefix}_spawned"
    read -r wake _ < <(add "$level" "$level_group" "$recruits" "$recruits_group" "$trigger")
    set_attrs "$level" "$level_group" "$wake" "TriggerAuto.0=${prefix}_spawn" "TriggerAuto.1=$spawn_delay"
fi

echo "made man \"$name\" (NPC_CRW_${prefix^^}): template $npc {$npc_guid}, SimNpc $simnpc {$sim_guid} of $globals#$globals_group"
echo "specialties $specialties (0x$(printf %X "$mask")), look $look_set/$look, joins on the message ${prefix}_join"

if [[ -n $mod ]]; then
    "$t" mods build "$game" -o "$mod" --name "Doctor crew member" --id doctor-crew --mod-version 1.0 \
        --description "A new made man for the family: example of doctor_toolkit (stage 82)" \
        --dir "$ws"
fi

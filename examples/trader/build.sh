#!/usr/bin/env bash
# A trader in Havana: walk up, talk, choose, pay, get the goods (stage 75).
#
#   examples/trader/build.sh GAME_COPY WORKSPACE [MOD.zip]
#
# GAME_COPY is a copy of the game, PC 1.0.789.0 (or a shadow made by
# `tools/game-bench.sh shadow`); nothing is written into it: every change is
# recorded in WORKSPACE. With MOD.zip the workspace is also built into a mod
# for the mod manager (`doctor-toolkit mods install GAME MOD.zip`, then
# `mods apply GAME`). Needs python3 for the dialogue tree (dialogue.py).
#
# What the mod adds (docs/analysis/handing-out.md has the reasons):
#
#   * a woman at the gate of Havana airport, next to the ticket agent: a
#     second SpawnPoint that asks for the SimNpc of the Miami ticket agent;
#   * her conversation, a stream of its own: a greeting and a menu
#       Pistol and bullets ($300) / Tommy gun ($1,500) / First aid ($200) /
#       Nothing today
#     The three purchases set the dialogue variables 0x23, 0x27 and 0x24: the
#     woman then sends the three messages her ChangeNPCSettings names;
#   * for each purchase a "bill": a copy of the Cash item worth nothing, given
#     to the player by a GiveItemRelay. Its pick-up message carries the player
#     in its data, which is what a MoneyAdaptor needs to take the money and a
#     GiveItemRelay with flag 8 needs to hand the goods to "whoever it is".
#
# Checked in the game: docs/analysis/game-bench.md §39. Known limits: the game
# has no test for "enough money", so with an empty pocket the trader takes
# what there is and still hands the goods; she has the name of the ticket
# agents (both are the same SimNpc name in the game's data).
#
# Lua can sell the same way: `SendMessage("doctor_trader_buy_a")`.
set -euo pipefail
game=${1:?game copy}
ws=${2:?workspace directory}
mod=${3:-}
t=${DOCTOR_TOOLKIT:-doctor-toolkit}
here=$(cd "$(dirname "$0")" && pwd)
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

logic=gf2/cuba/local                 # loaded all the time in Havana; SimGroup 714
logic_group=714
venue=gf2/cuba/local/yuri/high/venue_059_was_filler_64   # the airport gate; SimGroup 66
venue_group=66
globals=gf2/globals_global           # the items of the game; SimGroup 2955
globals_group=2955
dialogue_of_game=gf2/globalfolder/dlg/0823c6d1           # the ticket agent's conversation
dialogue=gf2/globalfolder/dlg/doctor_trader

# Objects of the game used as templates or referred to by GUID.
player=04bdc758713a65b141545a3032373358
pistol=d320c8321f543bee535a414b414c3230       # weapon_pistol_level_01
tommy_gun=4db4c75875125aba41545a3032373358    # weapon_automatic_level_01
health_pack=d7aec75838d199bf41545a3032373358  # Item "Health Pack", heals 500
miami_agent=8b9ac8365465ee0a5045525259363338  # SimNpc of the Miami ticket agent
relay_from=(gf2/cuba/global/objectives/chapter60-mission08-killingcastro/cp_739/cp_739_zonestream 0 16)
money_from=(mp_bonnie/mp_bonnie 442 23)       # a MoneyAdaptor of a multiplayer map
cash_object=2                                 # Item "Cash" in the globals
agent_settings=7 agent_spawn=9 agent_request=10   # the ticket agent's objects in the venue

# add GROUP_STREAM GROUP TEMPLATE_STREAM TEMPLATE_GROUP TEMPLATE_OBJECT [options]
# copies an object; the index and the GUID of the copy go to $object and $guid.
add() {
    local out
    out=$("$t" level add "$game" "$1" "$2" "$5" --template-stream "$3" --template-resource "$4" \
        --force --workspace-dir "$ws" "${@:6}")
    object=$(sed -n 's/^added object \([0-9]*\) (\([0-9a-f]*\)).*/\1/p' <<<"$out")
    guid=$(sed -n 's/^added object \([0-9]*\) (\([0-9a-f]*\)).*/\2/p' <<<"$out")
}
attrs() { "$t" simgroup-set "$game" "$1" "$2" "$3" --workspace-dir "$ws" "${@:4}" >/dev/null; }

# relay MESSAGE ITEM FLAGS [data]: a GiveItemRelay that gives ITEM on MESSAGE.
# FLAGS: 1 put it in the hand, 4 give even when there is one of the kind
# (ammunition, the next level, another bill). Target: the player by GUID, or
# with `data` the object in the data of the message.
relay() {
    add "$logic" "$logic_group" "${relay_from[@]}"
    attrs "$logic" "$logic_group" "$object" --set "GiveItemRelay.0=$1" --set "GiveItemRelay.1=$2" \
        --set "GiveItemRelay.2=1" --set "GiveItemRelay.3=$3"
    if [ "${4:-}" = data ]; then
        attrs "$logic" "$logic_group" "$object" --set "AttributeRelay.2=" --set "AttributeRelay.0=8"
    else
        attrs "$logic" "$logic_group" "$object" --set "AttributeRelay.2=$player"
    fi
}

# sale NAME PRICE ITEM FLAGS: message doctor_trader_buy_NAME -> a bill for the
# player -> doctor_trader_paid_NAME with the player -> the money and the goods.
sale() {
    add "$logic" "$logic_group" "$globals" "$globals_group" "$cash_object"
    attrs "$logic" "$logic_group" "$object" --set "Item.4=0" --set "Item.14=doctor_trader_paid_$1"
    relay "doctor_trader_buy_$1" "$guid" 4
    add "$logic" "$logic_group" "${money_from[@]}"
    attrs "$logic" "$logic_group" "$object" --set "EventAdaptor.0=doctor_trader_paid_$1" \
        --set "MoneyAdaptor.0=-$2" --set "MoneyAdaptor.1=5" --set "MoneyAdaptor.2="
    relay "doctor_trader_paid_$1" "$3" "$4" data
}
sale a 300 "$pistol" 5
sale b 1500 "$tommy_gun" 5
sale c 200 "$health_pack" 4

# The conversation: a stream with the animation banks of the ticket agent's
# dialogue and a tree of its own.
"$t" add-stream "$game" "$dialogue" --parent "$globals" --workspace-dir "$ws" >/dev/null
for bank in 0 1 2 3; do
    "$t" clone-resource "$game" "$dialogue_of_game" --index "$bank" --to "$dialogue" --workspace-dir "$ws" >/dev/null
done
"$t" dialogue "$game" "$dialogue_of_game" --json >"$tmp/ticket.json" 2>/dev/null
python3 "$here/dialogue.py" "$tmp/ticket.json" "$tmp/trader.json"
"$t" dialogue-import "$game" "$tmp/trader.json" --raw "$tmp/trader.dlg"
"$t" add-resource "$game" "$dialogue" --type dlg --data "$tmp/trader.dlg" --workspace-dir "$ws" >/dev/null
text() { "$t" text-add "$game" "$1" "$2" --lang "ru=$3" --workspace-dir "$ws" >/dev/null; }
text doctor_trader_a 'Pistol and bullets ($300)' 'Пистолет и патроны ($300)'
text doctor_trader_b 'Tommy gun ($1,500)' 'Автомат Томпсона ($1500)'
text doctor_trader_c 'First aid ($200)' 'Аптечка ($200)'
text doctor_trader_leave 'Nothing today' 'Сегодня ничего'

# The trader: a requester of the Miami agent, a spawn point 9 m from the
# ticket agent, and the settings that give her the conversation and the names
# of its three messages.
add "$venue" "$venue_group" "$venue" "$venue_group" "$agent_request"
request=$guid
attrs "$venue" "$venue_group" "$object" --set "SpecificNpcRequester.1=$miami_agent"
add "$venue" "$venue_group" "$venue" "$venue_group" "$agent_spawn" --position 290,0,378
attrs "$venue" "$venue_group" "$object" --set "SpawnPoint.0=$request" --set "SpawnPoint.6=doctor_trader_spawned"
add "$venue" "$venue_group" "$venue" "$venue_group" "$agent_settings"
attrs "$venue" "$venue_group" "$object" --set "ChangeNPCSettings.0=doctor_trader_spawned" \
    --set 'ChangeNPCSettings.27=gf2\globalfolder\dlg\doctor_trader' \
    --set "ChangeNPCSettings.3=doctor_trader_buy_a" --set "ChangeNPCSettings.4=doctor_trader_buy_b" \
    --set "ChangeNPCSettings.5=doctor_trader_buy_c"

echo "trader: recorded in $ws"
if [ -n "$mod" ]; then
    "$t" mods build "$game" --dir "$ws" --output "$mod" --id doctor-trader --name "Trader in Havana" \
        --description "A trader at the gate of Havana airport: pistol, Tommy gun, first aid" \
        --edition 1.0.789.0
fi

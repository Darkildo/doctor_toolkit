#!/usr/bin/env python3
"""Builds and checks data/gf2/tunables.toml, the curated game parameters of the
"Change game parameters" wizard.

Usage:
    doctor-toolkit logic-export --type simgroup -o <export_dir> <game_dir>
    doctor-toolkit search --limit 0 --keys <game_dir> type:metamodel stream:vehicles > <vehicles.txt>
    tunables.py build  <export_dir> <vehicles.txt> > data/gf2/tunables.toml
    tunables.py check  <export_dir> data/gf2/tunables.toml
    tunables.py survey <export_dir>

The input is the JSON that `logic-export` writes for every SimGroup: objects
with their class, GUID and attribute packets (class name, attribute index,
value type, value).

`build` resolves the curated list below against the game data and writes the
TOML. Objects are chosen by evidence that does not depend on the attribute
being described: the weapon set the `Player` object refers to (attributes
113-145 of the `Player` packet are GUIDs of `Gun`, `Item` and `ThrowableItem`
objects), the name an `Item` or `UpgradeData` object carries in its attribute
0, the name of a `Family` (attribute 1), the character model of a `CommonNPC`
(`PartedAnimated` attribute 2), the vehicle model of an `AICarBehavior`
(`Entity.CMD_SetModel`, the id of a metamodel in
`gf2/globalfolder/vehicles/assembly/vehicle_shared/<name>`), and the chase
configuration a law `Family` points to (`Family` 7 -> `ChaseConfigList` ->
four `ChaseConfig`).

`check` reads the TOML back and verifies that every entry resolves: the
resource exists, every object exists in it, has a packet of the class, and the
attribute holds a value of the entry's kind. It prints the current values and
whether the same object GUID also exists in other streams.

`survey` prints the evidence tables used in docs/analysis/tunables.md.
"""
import collections
import json
import os
import re
import sys

STREAM = "gf2/globals_global"
KEY = STREAM + ":simgroup:00000000"

# Player packet attributes holding the GUIDs of the player's weapons, levels 1-4.
PLAYER_GUNS = {
    "pistol": [114, 132, 137, 142],
    "magnum": [115, 133, 138, 143],
    "tommy_gun": [116, 134, 139, 144],
    "shotgun": [117, 135, 140, 145],
    "rifle": [113, 131, 136, 141],
}
GUN_LABEL = {"pistol": "Pistol", "magnum": "Magnum", "tommy_gun": "Tommy gun",
             "shotgun": "Shotgun", "rifle": "Rifle"}
PLAYER_THROWABLES = {"molotov": 119, "dynamite": 120, "bomb": 121}


def load_groups(root):
    groups = {}
    for dp, _, fns in os.walk(root):
        for fn in fns:
            if fn.endswith(".json") and "_simgroup_" in fn:
                d = json.load(open(os.path.join(dp, fn)))
                groups[d["stream"] + ":simgroup:" + d["id"]] = d
    return groups


def cname(x):
    return x.get("class_name") or x["class"]


def packet(o, cls):
    for p in o["packets"]:
        if cname(p) == cls:
            return {a["index"]: a for a in p["attrs"]}
    return None


def val(o, cls, idx):
    p = packet(o, cls) or {}
    a = p.get(idx)
    return a["value"] if a else None


class Group:
    def __init__(self, d):
        self.d = d
        self.objects = d["objects"]
        self.by_guid = {}
        for o in self.objects:
            self.by_guid.setdefault(o["guid"], []).append(o)

    def of_class(self, cls):
        return [o for o in self.objects if cname(o) == cls]

    def one(self, cls):
        objs = self.of_class(cls)
        assert len(objs) == 1, (cls, len(objs))
        return objs[0]

    def named(self, cls, pcls, idx, name):
        objs = [o for o in self.of_class(cls) if val(o, pcls, idx) == name]
        assert objs, (cls, name)
        return objs

    def player_ref(self, idx):
        g = val(self.one("Player"), "Player", idx)
        assert g in self.by_guid, (idx, g)
        return g

    def npc_model(self, pattern, hp=None):
        rx = re.compile(pattern)
        out = []
        for o in self.of_class("CommonNPC"):
            m = val(o, "PartedAnimated", 2) or ""
            if rx.match(m) and (hp is None or val(o, "StandardDamageComponent", 0) == hp):
                out.append(o)
        assert out, pattern
        return out

    def cars(self, models, model_names):
        out = []
        for o in self.of_class("AICarBehavior"):
            e = (packet(o, "Entity") or {}).get(1)
            if e and model_names.get(e.get("resource")) in models:
                out.append(o)
        assert out, models
        return out

    def chase_configs(self, level):
        """ChaseConfig of the given heat level (1-4) for the three law families."""
        out = []
        for law in ("miami_law", "queens_law", "cuban_law"):
            fam = self.named("Family", "Family", 1, law)[0]
            lst = self.by_guid[val(fam, "Family", 7)][0]
            assert cname(lst) == "ChaseConfigList"
            for i in range(4):
                cfg = self.by_guid[val(lst, "ChaseConfigList", i)][0]
                if val(cfg, "ChaseConfig", 16) == level:
                    out.append(cfg)
        assert len(out) == 3, level
        return out


def vehicle_models(path):
    """metamodel id -> model name from `search --keys type:metamodel stream:vehicles` output."""
    names = {}
    if path and os.path.exists(path):
        for line in open(path):
            parts = line.strip().split(":")
            if len(parts) == 3 and parts[1] == "metamodel":
                names[parts[2]] = parts[0].rsplit("/", 1)[1]
    return names


# Vehicle groups: (id part, label, models, description). Health is
# StandardDamageComponent attribute 0, named CMD_SetInitialHealth by the metamodels.
CAR_GROUPS = [
    ("sedans", "Sedans, taxi and wagon", ["sedan50_01", "sedan50_02", "sedan60_01", "sedan60_02", "sedan60_03", "taxi", "wagon60_01"]),
    ("sports_cars", "Sports cars", ["sportscar_01", "sportscar_02"]),
    ("muscle_cars", "Muscle cars", ["muscle60_01", "muscle60_02", "muscle60_03"]),
    ("vans", "Vans", ["van60_01", "van60_02"]),
    ("pickups", "Pickups and flatbed", ["truck_pickup", "truck_pickup_stepside", "truck_flatbed"]),
    ("heavy_trucks", "Heavy trucks", ["truck_heavy_racket", "truck_heavy_racket_queens", "truck_heavy_tow"]),
    ("family_cars", "Family (thug) cars", ["thug60"]),
    ("armored_family_cars", "Armored family cars", ["thug60_armored"]),
    ("police_cars", "Police cars and jeeps", ["police_miami", "police_queens", "jeep60_01"]),
    ("police_wagons", "Police wagons and army trucks", ["police_wagon", "truck_heavy_02_military"]),
]


def spec(g, models):
    """The curated list: dicts with the TOML fields and `objs` (list of objects)."""
    out = []

    def add(id, category, label, description, cls, attr, kind, objs, lo=None, hi=None):
        out.append(dict(id=id, category=category, label=label, description=description,
                        cls=cls, attr=attr, kind=kind, objs=objs, min=lo, max=hi))

    # --- weapons: the player's guns, all four upgrade levels per entry ---
    for w, refs in PLAYER_GUNS.items():
        objs = [g.by_guid[g.player_ref(i)][0] for i in refs]
        lab = GUN_LABEL[w]
        add("weapons.%s.damage" % w, "weapons", "%s: damage" % lab,
            "Damage of one bullet%s, probably; the player has 4500 health. "
            "One value for all four upgrade levels; Gun 88, 92 and 96 repeat it, probably as unused copies "
            "(the multiplayer rifle was rebalanced in attribute 6 only)." % (" (one pellet)" if w == "shotgun" else ""),
            "Gun", 6, "float", objs, 1.0, 100000.0)
        add("weapons.%s.clip_size" % w, "weapons", "%s: clip size" % lab,
            "Rounds per magazine, probably (grows with the upgrade level). One value for all four levels; "
            "Gun attributes 90, 94 and 98 repeat it.",
            "Gun", 7, "int", objs, 1, 999)
        add("weapons.%s.max_ammo" % w, "weapons", "%s: ammo carried" % lab,
            "Most ammunition the player can carry for this gun, probably (grows with the level). "
            "One value for all four levels; Gun attributes 91, 95 and 99 hold a similar number.",
            "Gun", 8, "int", objs, 1, 9999)
        add("weapons.%s.fire_interval" % w, "weapons", "%s: time between shots" % lab,
            "Seconds between shots, probably: lower is faster (the Tommy gun goes from 0.09 at level 1 to 0.06 at level 4). "
            "One value for all four levels; Gun 89, 93 and 97 repeat it (with older values for the rifles).",
            "Gun", 15, "float", objs, 0.01, 10.0)
    add("weapons.shotgun.pellets", "weapons", "Shotgun: pellets per shot",
        "Pellets fired per shot, probably: 10 for every shotgun, 1 for every other gun.",
        "Gun", 25, "int", [g.by_guid[g.player_ref(i)][0] for i in PLAYER_GUNS["shotgun"]], 1, 50)
    for t, ref in PLAYER_THROWABLES.items():
        o = g.by_guid[g.player_ref(ref)][0]
        add("weapons.%s.damage" % t, "weapons", "%s: explosion damage" % t.capitalize(),
            "Damage of the explosion, probably (molotov 2500, dynamite 5000, bomb 7500).",
            "ThrowableItem", 7, "float", [o], 0.0, 100000.0)
        add("weapons.%s.radius" % t, "weapons", "%s: blast radius" % t.capitalize(),
            "Outer radius of the blast in metres, probably; ThrowableItem attribute 5 holds a smaller inner radius.",
            "ThrowableItem", 6, "float", [o], 0.5, 50.0)

    # --- health ---
    add("health.player.health", "health", "Player: health",
        "Initial health of the player (CMD_SetInitialHealth). Named by the metamodels; 4500 against 1800 for a civilian.",
        "StandardDamageComponent", 0, "float", [g.one("Player")], 1.0, 1000000.0)
    for fid, lab, pat, hp, fam in [
        ("carmine_soldiers", "Carmine Rosato's soldiers", r"crosato_\d+_", 1800.0, "Carmine Rosato"),
        ("tony_soldiers", "Tony Rosato's soldiers", r"trosato_\d+_", 2000.0, "Tony Rosato"),
        ("granados_soldiers", "Granados soldiers", r"granadosassoc_\d+", 2200.0, "Granados"),
    ]:
        add("health.%s.health" % fid, "health", "%s: health" % lab,
            "Initial health of the %s family's rank-and-file NPC templates (CMD_SetInitialHealth), selected by character model; "
            "named lieutenants have their own templates and keep their values." % fam,
            "StandardDamageComponent", 0, "float", g.npc_model(pat, hp), 1.0, 1000000.0)

    # --- crew ---
    add("crew.corleone_associates.health", "crew", "Corleone associates: health",
        "Initial health of the 200 Corleone associate NPC templates (model corleoneassociate_*), probably the family soldiers "
        "that guard the player's businesses; hired made men use other templates.",
        "StandardDamageComponent", 0, "float", g.npc_model(r"corleoneassociate_\d+_", 2600.0), 1.0, 1000000.0)
    for name, attr, desc in [
        ("Bullet Proof Vests", 14, "Damage taken multiplier given by the Bullet Proof Vests crime-ring bonus, probably (0.8 = 20% less damage; 1.0 in every other upgrade)."),
        ("Armor Piercing Bullets", 16, "Damage dealt multiplier given by the Armor Piercing Bullets crime-ring bonus, probably (1.25; 1.0 in every other upgrade)."),
        ("Ammo Holsters", 17, "Ammunition capacity multiplier given by the Ammo Holsters crime-ring bonus, probably (1.5; 1.0 in every other upgrade)."),
        ("Brass Knuckles", 18, "Melee damage multiplier given by the Brass Knuckles crime-ring bonus, probably (1.25; 1.0 in every other upgrade)."),
    ]:
        slug = name.lower().replace(" ", "_")
        add("crew.%s.multiplier" % slug, "crew", "%s: bonus" % name, desc,
            "UpgradeData", attr, "float", g.named("UpgradeData", "UpgradeData", 0, name), 0.0, 10.0)
    add("crew.jail_break.multiplier", "crew", "Jail Break: time multiplier",
        "Multiplier of the Jail Break skill, probably on the time a jailed made man is out (0.5; 1.0 in every other upgrade).",
        "UpgradeData", 13, "float", g.named("UpgradeData", "UpgradeData", 0, "Jail Break"), 0.0, 10.0)

    # --- player upgrades ---
    for i, (what, attr) in enumerate([("molotovs", 21), ("dynamite", 22), ("bombs", 23)]):
        add("player.carry_more_explosives.max_%s" % what, "player", "Carry More Explosives: %s" % what,
            "How many %s the player can carry with the Carry More Explosives bonus, probably; every other upgrade stores "
            "the base count (4 molotovs, 3 dynamite, 1 bomb)." % what,
            "UpgradeData", attr, "int", g.named("UpgradeData", "UpgradeData", 0, "Carry More Explosives"), 1, 99)
    for lvl in (1, 2):
        add("player.health_regen_%d.multiplier" % lvl, "player", "Health Regen level %d: rate" % lvl,
            "Health regeneration multiplier of the Health Regen skill, probably (1.25 at level 1, 1.5 at level 2).",
            "UpgradeData", 31, "float", g.named("UpgradeData", "UpgradeData", 0, "Health Regen Level %d" % lvl), 0.0, 10.0)
    add("player.quick_heal.multiplier", "player", "Quick Heal: time multiplier",
        "Multiplier of the Quick Heal skill, probably on the time healing takes (0.5; 1.0 in every other upgrade).",
        "UpgradeData", 12, "float", g.named("UpgradeData", "UpgradeData", 0, "Quick Heal"), 0.0, 10.0)
    for lvl in (1, 2, 3):
        add("player.heart_%d.bonus" % lvl, "player", "Heart level %d: health bonus" % lvl,
            "Extra health of the Heart skill, probably in percent (10, 20, 30 at levels 1-3).",
            "UpgradeData", 35, "float", g.named("UpgradeData", "UpgradeData", 0, "Heart Level %d" % lvl), 0.0, 1000.0)
    for lvl in (1, 2):
        add("player.heavy_hitter_%d.multiplier" % lvl, "player", "Heavy Hitter level %d: damage" % lvl,
            "Melee damage multiplier of the Heavy Hitter skill, probably (1.25 at level 1, 1.5 at level 2).",
            "UpgradeData", 32, "float", g.named("UpgradeData", "UpgradeData", 0, "Heavy Hitter Level %d" % lvl), 0.0, 10.0)

    # --- economy ---
    rlc = g.named("UpgradeData", "UpgradeData", 0, "Reduced Law Costs")
    add("economy.reduced_law_costs.multiplier_a", "economy", "Reduced Law Costs: cost multiplier A",
        "First of two cost multipliers of the Reduced Law Costs crime-ring bonus (0.5), probably the price of bribes; "
        "which law cost each one scales is not known.",
        "UpgradeData", 19, "float", rlc, 0.0, 10.0)
    add("economy.reduced_law_costs.multiplier_b", "economy", "Reduced Law Costs: cost multiplier B",
        "Second cost multiplier of the Reduced Law Costs crime-ring bonus (0.5), probably another law cost; "
        "which one is not known.",
        "UpgradeData", 20, "float", rlc, 0.0, 10.0)
    banks = g.of_class("Bank")
    add("economy.banks.cash_a", "economy", "Banks: cash value A",
        "Money amount of the four Bank objects (10000), probably the larger of two cash amounts of a bank job; not verified.",
        "Bank", 0, "int", banks, 0, 10000000)
    add("economy.banks.cash_b", "economy", "Banks: cash value B",
        "Second money amount of the four Bank objects (5000), probably the smaller cash amount of a bank job; not verified.",
        "Bank", 1, "int", banks, 0, 10000000)

    # --- police ---
    for lvl, v in ((2, 75.0), (3, 150.0), (4, 225.0)):
        add("police.wanted_%d.heat_threshold" % lvl, "police", "Wanted level %d: heat threshold" % lvl,
            "Heat at which the police chase escalates to level %d, probably; the three cities' ChaseConfig objects all hold %g, "
            "and attribute 11 of the level below holds the same boundary." % (lvl, v),
            "ChaseConfig", 10, "float", g.chase_configs(lvl), 0.0, 100000.0)
    add("police.wanted_4.heat_max", "police", "Wanted level 4: heat cap",
        "Upper heat bound of the highest chase level (1000), probably the most heat the player can have.",
        "ChaseConfig", 11, "float", g.chase_configs(4), 0.0, 100000.0)
    add("police.officers.health", "police", "Police officers: health",
        "Initial health of the Miami and New York police officer and Cuban police sergeant templates (2200, CMD_SetInitialHealth).",
        "StandardDamageComponent", 0, "float",
        g.npc_model(r"(miami_police|nyc_police|cuban_police_sergeant)_\d+", 2200.0), 1.0, 1000000.0)
    add("police.sergeants_swat.health", "police", "Police sergeants and SWAT: health",
        "Initial health of the Miami and New York police sergeant and SWAT templates (2750, CMD_SetInitialHealth).",
        "StandardDamageComponent", 0, "float",
        g.npc_model(r"(miami_seargent|nyc_seargent|miami_swat|nyc_swat)_\d+", 2750.0), 1.0, 1000000.0)

    # --- vehicles ---
    for vid, lab, ms in CAR_GROUPS:
        objs = g.cars(set(ms), models)
        add("vehicles.%s.health" % vid, "vehicles", "%s: health" % lab,
            "Initial health of the %s vehicle templates (CMD_SetInitialHealth on StandardDamageComponent); models %s."
            % (lab.lower(), ", ".join(ms)),
            "StandardDamageComponent", 0, "float", objs, 1.0, 10000000.0)
    for vid, lab, ms in [("family_cars", "Family (thug) cars", ["thug60"]),
                         ("armored_family_cars", "Armored family cars", ["thug60_armored"])]:
        add("vehicles.%s.damage_factor" % vid, "vehicles", "%s: damage factor" % lab,
            "VehicleDamageComponent attribute 14, probably how strongly the car takes damage: 0.6 sports cars, 0.55 sedans, "
            "0.4 family cars, 0.1 armored cars and heavy trucks.",
            "VehicleDamageComponent", 14, "float", g.cars(set(ms), models), 0.0, 10.0)

    # --- world: rival families ---
    for fam, lab in [("rosato_carmine", "Carmine Rosato"), ("rosato_tony", "Tony Rosato"), ("granados", "Granados"),
                     ("mangano", "Mangano"), ("almeida", "Almeida")]:
        add("world.%s.strength" % fam, "world", "%s family: strength" % lab,
            "Family attribute 8, probably a strength or difficulty multiplier: 1.0 for most families, rising along the story "
            "(Carmine 1.0, Tony 1.05, Granados 1.1, Mangano 1.2, Almeida 1.4).",
            "Family", 8, "float", g.named("Family", "Family", 1, fam), 0.0, 10.0)
    return out


def fmt_value(v, kind):
    if kind == "float":
        return repr(float(v))
    return str(int(v))


def build(root, models_path):
    groups = load_groups(root)
    g = Group(groups[KEY])
    models = vehicle_models(models_path)
    entries = spec(g, models)
    ids = [e["id"] for e in entries]
    assert len(ids) == len(set(ids)), "duplicate ids"
    print("# Curated game parameters for the \"Change game parameters\" wizard.")
    print("# Built by research/tunables.py; evidence in docs/analysis/tunables.md.")
    print("version = 1")
    for e in entries:
        print()
        print("[[tunable]]")
        print("id = %s" % json.dumps(e["id"]))
        print("category = %s" % json.dumps(e["category"]))
        print("label = %s" % json.dumps(e["label"]))
        print("description = %s" % json.dumps(e["description"]))
        print('source = "simgroup"')
        print("resource = %s" % json.dumps(KEY))
        print("class = %s" % json.dumps(e["cls"]))
        print("attribute = %d" % e["attr"])
        objs = e["objs"]
        guids = [o["guid"] for o in objs]
        if len(guids) <= 4:
            notes = [obj_label(o) for o in objs]
            print("objects = [%s]  # %s" % (", ".join(json.dumps(x) for x in guids), ", ".join(notes)))
            if len(set(notes)) == len(notes):
                print("object_names = [%s]" % ", ".join(json.dumps(n) for n in notes))
        else:
            print("objects = [  # %d objects" % len(guids))
            for k in range(0, len(guids), 3):
                print("  %s," % ", ".join(json.dumps(x) for x in guids[k:k + 3]))
            print("]")
        print("kind = %s" % json.dumps(e["kind"]))
        if e["min"] is not None:
            print("min = %s" % fmt_value(e["min"], e["kind"]))
        if e["max"] is not None:
            print("max = %s" % fmt_value(e["max"], e["kind"]))


def obj_label(o):
    for cls, idx in (("Item", 0), ("UpgradeData", 0), ("Family", 1), ("PartedAnimated", 2), ("Bank", 2)):
        v = val(o, cls, idx)
        if isinstance(v, str):
            return v
    return cname(o)


def check(root, toml_path):
    import tomllib
    doc = tomllib.load(open(toml_path, "rb"))
    assert doc.get("version") == 1
    groups = load_groups(root)
    everywhere = collections.defaultdict(set)
    for key, d in groups.items():
        for o in d["objects"]:
            everywhere[o["guid"]].add(d["stream"])
    bad = 0
    seen = set()
    cats = collections.Counter()
    for t in doc["tunable"]:
        problems = []
        if t["id"] in seen:
            problems.append("duplicate id")
        seen.add(t["id"])
        cats[t["category"]] += 1
        if t["category"] not in ("weapons", "economy", "health", "police", "vehicles", "player", "crew", "world"):
            problems.append("unknown category")
        if t["source"] != "simgroup":
            problems.append("only simgroup entries are checked here")
        d = groups.get(t["resource"])
        if d is None:
            print("FAIL %s: no resource %s" % (t["id"], t["resource"]))
            bad += 1
            continue
        g = Group(d)
        objs = []
        if "objects" in t:
            for guid in t["objects"]:
                found = g.by_guid.get(guid)
                if not found:
                    problems.append("no object %s" % guid)
                elif len(found) > 1:
                    problems.append("object %s is not unique in the group" % guid)
                else:
                    objs.append(found[0])
        else:
            objs = [o for o in g.objects if packet(o, t["class"]) is not None]
        values = collections.Counter()
        others = set()
        for o in objs:
            p = packet(o, t["class"])
            if p is None:
                problems.append("object %s has no %s packet" % (o["guid"], t["class"]))
                continue
            a = p.get(t["attribute"])
            if a is None:
                problems.append("object %s: %s %d is default (not stored)" % (o["guid"], t["class"], t["attribute"]))
                continue
            want = {"int": "int", "bool": "int", "float": "float"}[t["kind"]]
            if a["type"] != want:
                problems.append("object %s: attribute is %s, not %s" % (o["guid"], a["type"], t["kind"]))
            v = a["value"]
            if t["kind"] == "bool" and v not in (0, 1):
                problems.append("bool value %r" % v)
            if "min" in t and v < t["min"] or "max" in t and v > t["max"]:
                problems.append("current value %r outside [%s, %s]" % (v, t.get("min"), t.get("max")))
            values[round(v, 4) if isinstance(v, float) else v] += 1
            others |= everywhere[o["guid"]] - {d["stream"]}
        status = "FAIL" if problems else "ok"
        bad += bool(problems)
        vs = ", ".join("%g×%d" % (v, n) if n > 1 else "%g" % v for v, n in sorted(values.items()))
        print("%-4s %-46s %-24s %3d  %3d obj  %s%s" % (status, t["id"], t["class"], t["attribute"], len(objs), vs,
                                                      "  [also in %d other stream(s): %s]" % (len(others), ", ".join(sorted(others))[:120]) if others else ""))
        for p in problems[:5]:
            print("     ", p)
    print("%d entries, %d failing; per category: %s" % (len(doc["tunable"]), bad, dict(cats)))
    return bad == 0


def survey(root):
    """Evidence tables for the analysis document."""
    groups = load_groups(root)
    g = Group(groups[KEY])
    print("== player weapons (Player attribute -> Gun): level, type, Gun 4 5 6 7 8 9 10 15 25 | 88 89 90 91")
    for w, refs in PLAYER_GUNS.items():
        for i in refs:
            o = g.by_guid[g.player_ref(i)][0]
            row = [val(o, "Gun", k) for k in (1, 2, 4, 5, 6, 7, 8, 9, 10, 15, 25, 88, 89, 90, 91)]
            print("Player.%d %-28s %s" % (i, val(o, "Item", 0), " ".join("%g" % v if v is not None else "-" for v in row)))
    print("== Gun copies across the corpus: objects where 6 != 88/92/96, 15 != 89, 7 != 90, 8 != 91")
    diff = collections.Counter()
    total = 0
    for d in groups.values():
        for o in d["objects"]:
            if cname(o) != "Gun":
                continue
            total += 1
            for a, bs in ((6, (88, 92, 96)), (15, (89, 93, 97)), (7, (90, 94, 98)), (8, (91, 95, 99))):
                if any(val(o, "Gun", a) != val(o, "Gun", b) for b in bs):
                    diff[a] += 1
    print(total, "Gun objects;", dict(diff))
    print("== throwables (ThrowableItem 2 5 6 7 15)")
    for t, ref in PLAYER_THROWABLES.items():
        o = g.by_guid[g.player_ref(ref)][0]
        print(t, val(o, "Item", 0), [val(o, "ThrowableItem", k) for k in (2, 5, 6, 7, 15)])
    print("== UpgradeData non-1.0 multipliers (attributes 12-62) and 21-23")
    for o in g.of_class("UpgradeData"):
        p = packet(o, "UpgradeData")
        mods = {k: a["value"] for k, a in p.items() if 12 <= k <= 62 and a["value"] != 1.0 and a["type"] == "float"}
        caps = tuple(p[k]["value"] if k in p else None for k in (21, 22, 23))
        print("%-38s cat=%s id=%s lvl=%s %s caps=%s" % (val(o, "UpgradeData", 0), val(o, "UpgradeData", 3),
                                                        val(o, "UpgradeData", 4), val(o, "UpgradeData", 5),
                                                        {k: round(v, 3) for k, v in mods.items()}, caps))
    print("== chase configs (ChaseConfig 2 3 7 8 10 11 16) per law family")
    for law in ("miami_law", "queens_law", "cuban_law"):
        for lvl in (1, 2, 3, 4):
            for cfg in g.chase_configs(lvl):
                pass
        fam = g.named("Family", "Family", 1, law)[0]
        lst = g.by_guid[val(fam, "Family", 7)][0]
        for i in range(4):
            cfg = g.by_guid[val(lst, "ChaseConfigList", i)][0]
            print(law, [val(cfg, "ChaseConfig", k) for k in (2, 3, 7, 8, 10, 11, 16)])
    print("== rival families (Family 8 18 19 20 21 23)")
    for o in g.of_class("Family"):
        print("%-18s" % val(o, "Family", 1), [val(o, "Family", k) for k in (8, 18, 19, 20, 21, 23)])
    print("== banks (Bank 0 1 2 3)")
    for o in g.of_class("Bank"):
        print([val(o, "Bank", k) for k in (0, 1, 2, 3)])


def main(argv):
    if len(argv) >= 2 and argv[0] == "build":
        build(argv[1], argv[2] if len(argv) > 2 else None)
    elif len(argv) == 3 and argv[0] == "check":
        sys.exit(0 if check(argv[1], argv[2]) else 1)
    elif len(argv) == 2 and argv[0] == "survey":
        survey(argv[1])
    else:
        print(__doc__)
        sys.exit(2)


if __name__ == "__main__":
    main(sys.argv[1:])

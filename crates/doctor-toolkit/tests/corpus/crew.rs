//! Stage 82: the made men of the player's family as the SimGroups hold them
//! (docs/analysis/crew.md §2–§4, §10).

use std::collections::{BTreeMap, BTreeSet};

use doctor_toolkit::game::GameInstance;
use doctor_toolkit::hash::string_id;
use doctor_toolkit::logic_graph::{ScopeSpec, scope};
use doctor_toolkit::resource::simgroup::Value;

/// A GUID as a map key.
type Key = [u8; 16];

/// The recruits: 81 `CommonNPC` templates named `NPC_CRW_0001`…`0081`, each
/// with a `SimNpc` of its own; the hash of the text of the name is
/// `sdbm("name_" + NPC.2)`; the specialties are bits 0x04…0x80 of
/// `NPCCrewComponent.0`; every template belongs to the family `corleone`, with
/// rank 1 (the bruisers: 3). The game places 74 of them in the world with 80
/// `SpecificNpcRequester` objects and has 20 `AddPlayerFamilyMember` objects
/// with ranks 3…5; `UnlockCheck` guards the recruits of the add-ons.
#[test]
fn recruits_and_who_points_at_them() {
    let Some(dir) = super::game_dir() else { return };
    let game = GameInstance::open(&dir).unwrap();
    let (index, _) = game.build_index(|_| {});
    let locations = scope::groups(&game, &index, &ScopeSpec::Game).unwrap();
    let (loaded, errors) = scope::load(&game, None, &locations);
    assert!(errors.is_empty(), "{:?}", errors.first());

    let class = |name: &str| string_id(name);
    let (npc, crew, sentient, sim_npc) = (
        class("NPC"),
        class("NPCCrewComponent"),
        class("Sentient"),
        class("SimNpc"),
    );
    let (requester, add_member, unlock) = (
        class("SpecificNpcRequester"),
        class("AddPlayerFamilyMember"),
        class("UnlockCheck"),
    );

    // Templates by GUID: name, specialties.
    let mut templates: BTreeMap<Key, (String, u32)> = BTreeMap::new();
    let mut sims: BTreeMap<Key, Key> = BTreeMap::new();
    let mut requested: Vec<Key> = Vec::new();
    let mut added: Vec<(Key, i32, i32)> = Vec::new();
    let mut unlock_checks = Vec::new();
    for g in &loaded {
        for o in &g.group.objects {
            let attr = |packet: u32, i: u16| {
                g.group
                    .object_packets(o)
                    .find(|p| p.class.0 == packet)
                    .and_then(|p| p.attrs.iter().find(|a| a.index == i))
                    .map(|a| &a.value)
            };
            let int = |packet: u32, i: u16| match attr(packet, i) {
                Some(Value::Int(n)) => *n,
                _ => 0,
            };
            if let Some(Value::String(name)) = attr(npc, 2)
                && name.starts_with("NPC_CRW_")
            {
                assert_eq!(g.stream.replace('\\', "/"), "gf2/globals_global", "{name}");
                let Some(Value::Hash(text)) = attr(npc, 18) else {
                    panic!("{name}: no hash of the name")
                };
                assert_eq!(
                    text.0,
                    string_id(&format!("name_{}", name.to_lowercase())),
                    "{name}"
                );
                let Some(Value::Hash(family)) = attr(sentient, 2) else {
                    panic!("{name}: no family")
                };
                assert_eq!(family.0, string_id("corleone"), "{name}");
                let word = int(crew, 0) as u32;
                // Rank 1, but 3 for the eleven who are bruisers and nothing else.
                let rank = if word & 0xFC == 0x80 { 3 } else { 1 };
                assert_eq!(int(sentient, 1), rank, "{name}: rank");
                assert_eq!(word & !0xFC, 0x101, "{name}: bits besides the specialties");
                assert_ne!(word & 0xFC, 0, "{name}: no specialty");
                templates.insert(o.guid.0, (name.clone(), word & 0xFC));
            }
            if o.class == sim_npc
                && let Some(Value::Guid(template)) = attr(sim_npc, 1)
            {
                sims.insert(o.guid.0, template.0);
            }
            if o.class == requester
                && let Some(Value::Guid(sim)) = attr(requester, 1)
            {
                requested.push(sim.0);
            }
            if o.class == add_member {
                let Some(Value::Guid(sim)) = attr(add_member, 1) else {
                    panic!("AddPlayerFamilyMember without a SimNpc")
                };
                added.push((sim.0, int(add_member, 2), int(add_member, 3)));
            }
            if o.class == unlock {
                unlock_checks.push(int(unlock, 2));
            }
        }
    }

    let names: Vec<&str> = templates.values().map(|(n, _)| n.as_str()).collect();
    let expected: Vec<String> = (1..=81).map(|n| format!("NPC_CRW_{n:04}")).collect();
    let mut sorted = names.clone();
    sorted.sort_unstable();
    assert_eq!(sorted, expected);
    // One, two or four specialties.
    let mut by_count = BTreeMap::new();
    for (_, bits) in templates.values() {
        *by_count.entry(bits.count_ones()).or_insert(0) += 1;
    }
    assert_eq!(
        by_count.into_iter().collect::<Vec<_>>(),
        [(1, 66), (2, 14), (4, 1)]
    );

    // Every template has exactly one SimNpc.
    let crew_sims: BTreeMap<Key, Key> = sims
        .into_iter()
        .filter(|(_, template)| templates.contains_key(template))
        .collect();
    assert_eq!(crew_sims.len(), 81);
    assert_eq!(crew_sims.values().collect::<BTreeSet<_>>().len(), 81);

    let placed: Vec<&Key> = requested
        .iter()
        .filter(|g| crew_sims.contains_key(*g))
        .collect();
    assert_eq!(placed.len(), 80);
    assert_eq!(placed.iter().collect::<BTreeSet<_>>().len(), 74);

    assert_eq!(added.len(), 20);
    for (sim, rank, specialties) in &added {
        assert!(
            crew_sims.contains_key(sim),
            "AddPlayerFamilyMember of a stranger"
        );
        assert!((3..=5).contains(rank), "rank {rank}");
        assert!(
            *specialties != 0 && specialties & !0xFC == 0,
            "specialties {specialties:#x}"
        );
    }
    unlock_checks.sort_unstable();
    assert_eq!(unlock_checks, [0, 0, 1, 1]);
}

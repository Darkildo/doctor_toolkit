//! Stage 73: the typed game data of `data/gf2/` (message catalogue, value
//! lists, meanings of attributes) against the game files it was read from.

use std::collections::{BTreeSet, HashMap, HashSet};

use doctor_toolkit::game::GameInstance;
use doctor_toolkit::game_types::{self, MessageCatalog, MessageId, SemanticType, Usage};
use doctor_toolkit::hash::{sdbm, string_id};
use doctor_toolkit::logic_graph::{MessageIndex, ScopeSpec, scope};
use doctor_toolkit::resource::simgroup::{Guid, Value};

/// `data/gf2/game_messages.tsv` is the game: the same messages with the
/// same numbers of senders and listeners as `MessageIndex` reads from the
/// files, and the counts the documents quote.
#[test]
fn the_message_catalogue_is_the_messages_of_the_game() {
    let Some(dir) = super::game_dir() else { return };
    let game = GameInstance::open(&dir).unwrap();
    let (index, _) = game.build_index(|_| {});
    let messages = MessageIndex::build(&game, &index, None);
    assert!(messages.errors.is_empty(), "{:?}", messages.errors.first());
    let catalog = MessageCatalog::gf2();
    assert_eq!(messages.by_hash.len(), catalog.len());
    let mut failures = Vec::new();
    for (hash, sites) in &messages.by_hash {
        let Some(m) = catalog.get(*hash) else {
            failures.push(format!("{} is not in the catalogue", sites.name));
            continue;
        };
        let total = |uses: Vec<game_types::PinUse>| -> usize {
            uses.iter().map(|u| u.count as usize).sum()
        };
        if total(m.senders()) != sites.senders.len()
            || total(m.listeners()) != sites.listeners.len()
        {
            failures.push(format!(
                "{}: catalogue {} sender(s) and {} listener(s), game {} and {}",
                m.name,
                total(m.senders()),
                total(m.listeners()),
                sites.senders.len(),
                sites.listeners.len()
            ));
        }
        if !m.name.eq_ignore_ascii_case(&sites.name) {
            failures.push(format!("{} and {} have one hash", m.name, sites.name));
        }
    }
    assert!(
        failures.is_empty(),
        "{:#?}",
        &failures[..failures.len().min(20)]
    );

    let stats = messages.stats();
    eprintln!("whole game: {stats:?}");
    assert_eq!(
        (
            stats.messages,
            stats.senders,
            stats.listeners,
            stats.connected,
            stats.from_engine,
            stats.to_engine,
            stats.never_sent,
            stats.never_heard
        ),
        (42_087, 58_320, 72_241, 33_113, 150, 34, 1_001, 7_789)
    );
    // The groups of the open enum and the usage the catalogue reports.
    let mut kinds: HashMap<&str, usize> = HashMap::new();
    let mut usage: HashMap<Usage, usize> = HashMap::new();
    for m in catalog.iter() {
        let id = MessageId::from_hash(m.hash);
        *kinds.entry(id.kind()).or_default() += 1;
        *usage.entry(id.info().usage).or_default() += 1;
    }
    assert_eq!(kinds["engine"], 198);
    assert_eq!(kinds["game"], 41_889);
    assert_eq!(game_types::message::engine_messages().len(), 645);
    assert_eq!(usage[&Usage::SentAndHeard], stats.connected);
    assert_eq!(usage[&Usage::HeardFromEngine], stats.from_engine);
    assert_eq!(usage[&Usage::SentToEngine], stats.to_engine);
    assert_eq!(usage[&Usage::HeardNeverSent], stats.never_sent);
    assert_eq!(usage[&Usage::SentNeverHeard], stats.never_heard);
    // No name of the game collides with another one.
    for m in catalog.iter().take(2000) {
        assert_eq!(game_types::message::collision(m.name), None, "{}", m.name);
    }
}

/// The value lists hold what the objects of the game hold, and every typed
/// attribute has in the files exactly the values `attr_types.toml` counts.
#[test]
fn value_lists_and_attribute_types_match_the_game() {
    let Some(dir) = super::game_dir() else { return };
    let game = GameInstance::open(&dir).unwrap();
    let (index, _) = game.build_index(|_| {});
    let locations = scope::groups(&game, &index, &ScopeSpec::Game).unwrap();
    let (loaded, errors) = scope::load(&game, None, &locations);
    assert!(errors.is_empty(), "{:?}", errors.first());
    assert_eq!(loaded.len(), 2006);

    // Resource ids by type, and the packets of every object by GUID.
    let mut resources: HashSet<(u32, u32)> = HashSet::new();
    for e in &index.entries {
        resources.insert((e.header.type_id, e.header.ids[3]));
    }
    let mut packets_of: HashMap<Guid, Vec<HashSet<u32>>> = HashMap::new();
    for g in &loaded {
        for o in &g.group.objects {
            packets_of
                .entry(o.guid)
                .or_default()
                .push(g.group.object_packets(o).map(|p| p.class.0).collect());
        }
    }

    let (family, city, building, item) = (
        string_id("Family"),
        string_id("City"),
        string_id("Building"),
        string_id("Item"),
    );
    let mut families = BTreeSet::new();
    let mut cities = BTreeSet::new();
    let mut venues = BTreeSet::new();
    let mut items = BTreeSet::new();
    // (packet class, attribute) → (values that fit the type, values).
    let mut seen: HashMap<(u32, u16), (u32, u32)> = HashMap::new();
    for g in &loaded {
        for o in &g.group.objects {
            for p in g.group.object_packets(o) {
                let class = p.class.0;
                for a in &p.attrs {
                    match (&a.value, a.index) {
                        (Value::String(s), 1) if class == family && o.class == family => {
                            families.insert(s.clone());
                        }
                        (Value::String(s), 0) if class == city && o.class == city => {
                            cities.insert(s.clone());
                        }
                        (Value::Int(i), 0) if class == building && o.class == building => {
                            venues.insert(i64::from(*i));
                        }
                        (Value::String(s), 0) if class == item && !s.is_empty() => {
                            items.insert(s.clone());
                        }
                        _ => {}
                    }
                    let Some(t) = game_types::attr_type(class, a.index) else {
                        continue;
                    };
                    let fits = match (t.ty, &a.value) {
                        (SemanticType::Enum(key), v) => {
                            game_types::enum_def(key).unwrap().find(v).is_some()
                        }
                        (SemanticType::Ref(name), Value::Guid(guid)) => {
                            let want = game_types::class_hash(name);
                            packets_of
                                .get(guid)
                                .is_some_and(|copies| copies.iter().any(|c| c.contains(&want)))
                        }
                        (SemanticType::Resource(tag), Value::Guid(guid)) => guid
                            .resource_id()
                            .is_some_and(|id| resources.contains(&(sdbm(tag.as_bytes()), id))),
                        (SemanticType::Bool, Value::Int(i)) => matches!(i, 0 | 1),
                        (SemanticType::Seconds | SemanticType::Money, v) => {
                            matches!(v, Value::Int(_) | Value::Float(_))
                        }
                        _ => false,
                    };
                    let entry = seen.entry((class, a.index)).or_default();
                    entry.0 += u32::from(fits);
                    entry.1 += 1;
                }
            }
        }
    }

    let names = |key: &str| -> BTreeSet<String> {
        game_types::enum_def(key)
            .unwrap()
            .values
            .iter()
            .filter_map(|v| v.name.map(str::to_owned))
            .collect()
    };
    assert_eq!(families, names("family"));
    assert_eq!(families.len(), 28);
    assert_eq!(cities, names("city"));
    assert_eq!(items, names("weapon"));
    let listed: BTreeSet<i64> = game_types::enum_def("venue")
        .unwrap()
        .values
        .iter()
        .filter_map(|v| v.value)
        .collect();
    assert_eq!(venues, listed);

    let mut failures = Vec::new();
    for (class, index, t) in game_types::attr_types() {
        let found = seen.get(&(*class, *index)).copied().unwrap_or((0, 0));
        if found != t.seen {
            failures.push(format!(
                "{class:08x}.{index} {}: attr_types.toml says {:?}, the game has {found:?}",
                t.ty, t.seen
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} failure(s): {:#?}",
        failures.len(),
        &failures[..failures.len().min(20)]
    );
    eprintln!(
        "{} typed attributes checked against {} SimGroups",
        game_types::attr_types().len(),
        loaded.len()
    );
}

/// Stage 78: `data/gf2/bus_sites.tsv` is the executable — every row is a
/// call of the function its column names, the broadcast of the bus has the
/// 688 calls the documents quote, and every name the code gives to a message
/// variable is a string of the executable and a message of the engine list.
#[test]
fn the_bus_sites_are_calls_of_the_executable() {
    let Some(dir) = super::game_dir() else { return };
    let Ok(exe) = std::fs::read(dir.join("godfather2.exe")) else {
        return;
    };
    // Sections of PC 1.0.789.0 lie in the file at their virtual offset.
    const BASE: u32 = 0x0040_0000;
    const TEXT: std::ops::Range<u32> = 0x0040_1000..0x00D5_435A;
    let call_at = |site: u32| -> Option<u32> {
        let at = (site - BASE) as usize;
        (exe.get(at) == Some(&0xE8)).then(|| {
            let rel = i32::from_le_bytes(exe[at + 1..at + 5].try_into().unwrap());
            site.wrapping_add(5).wrapping_add(rel as u32)
        })
    };
    let calls_of = |target: u32| -> usize {
        TEXT.clone()
            .filter(|&site| call_at(site) == Some(target))
            .count()
    };
    assert_eq!(calls_of(0x0040_8A00), 688);
    assert_eq!(calls_of(0x0040_8BF0), 93);
    assert_eq!(calls_of(0x0040_8B80), 11);
    assert_eq!(calls_of(0x0040_8BB0), 23);
    assert_eq!(calls_of(0x0044_0590), 62);
    let text = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/gf2/bus_sites.tsv"),
    )
    .unwrap();
    let hex = |s: &str| u32::from_str_radix(s.trim_start_matches("0x"), 16).unwrap();
    let mut sends = HashSet::new();
    let mut rows = 0;
    for line in text.lines().filter(|l| !l.starts_with('#')) {
        let f: Vec<&str> = line.split('\t').collect();
        assert_eq!(f.len(), 7, "{line}");
        let (site, target) = (hex(f[1]), hex(f[3]));
        assert_eq!(call_at(site), Some(target), "{line}");
        rows += 1;
        match f[0] {
            "send" if target == 0x0040_8A00 => {
                sends.insert(site);
            }
            "name" => {
                let name = f[4].as_bytes();
                assert!(
                    exe.windows(name.len() + 1)
                        .any(|w| &w[..name.len()] == name && w[name.len()] == 0),
                    "{line}"
                );
                assert!(
                    matches!(MessageId::from_name(f[4]), MessageId::Engine(_)),
                    "{line}"
                );
            }
            _ => {}
        }
    }
    // Every broadcast call of the executable has its row.
    assert_eq!(sends.len(), 688);
    assert_eq!(rows, 2747);
}

/// Stage 79, `0x00446BF0`: the loader does not walk the `Base` packet of a
/// stored object, it reads two bytes of the object record instead — bit 1
/// of the flag byte says the object has the packet, and the next byte is a
/// copy of its attribute 0 (the spawn flags; 0 when it is not stored).
#[test]
fn the_second_flag_byte_of_an_object_is_its_base_attribute() {
    let Some(dir) = super::game_dir() else { return };
    let game = GameInstance::open(&dir).unwrap();
    let (index, _) = game.build_index(|_| {});
    let locations = scope::groups(&game, &index, &ScopeSpec::Game).unwrap();
    let (loaded, errors) = scope::load(&game, None, &locations);
    assert!(errors.is_empty(), "{:?}", errors.first());
    let base = string_id("Base");
    let (mut objects, mut with_base, mut set) = (0u32, 0u32, 0u32);
    for g in &loaded {
        for o in &g.group.objects {
            objects += 1;
            let has_base = g.group.object_packets(o).any(|p| p.class.0 == base);
            let value = match g.group.attr(o, base, 0) {
                Some(Value::Int(v)) => *v as u32,
                Some(other) => panic!("Base attribute 0 is {other:?}"),
                None => 0,
            };
            assert_eq!(o.flags & 2 != 0, has_base, "{} {}", g.stream, o.guid);
            assert_eq!(u32::from(o.flags2), value, "{} {}", g.stream, o.guid);
            with_base += u32::from(has_base);
            set += u32::from(o.flags2 != 0);
        }
    }
    assert_eq!((objects, with_base, set), (111_518, 108_349, 100_032));
}

/// Stage 80, `0x0043AFF0`: the engine's iterator of attribute packets has a
/// second path, for object records that are not packed (bit 0 of the flag
/// byte clear: a list of tagged entries instead of masks and values). The
/// files of the game hold no such record, so that path — and the same path
/// of the platform module's reader — is known by the code only.
#[test]
fn every_object_record_of_the_game_is_packed() {
    let Some(dir) = super::game_dir() else { return };
    let game = GameInstance::open(&dir).unwrap();
    let (index, _) = game.build_index(|_| {});
    let locations = scope::groups(&game, &index, &ScopeSpec::Game).unwrap();
    let (loaded, errors) = scope::load(&game, None, &locations);
    assert!(errors.is_empty(), "{:?}", errors.first());
    let mut by_flags = std::collections::BTreeMap::new();
    for g in &loaded {
        for o in &g.group.objects {
            assert!(o.flags & 1 != 0, "{} {}", g.stream, o.guid);
            *by_flags.entry(o.flags).or_insert(0u32) += 1;
        }
    }
    // Packed; packed with the packet `Base`; the same with components.
    assert_eq!(
        by_flags.into_iter().collect::<Vec<_>>(),
        [(0x01, 3_169), (0x03, 83_765), (0x0B, 24_584)]
    );
}

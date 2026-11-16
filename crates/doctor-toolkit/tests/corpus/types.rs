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
        (41_872, 57_881, 72_633, 32_933, 150, 34, 1_182, 7_573)
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
    assert_eq!(kinds["game"], 41_674);
    assert_eq!(game_types::message::engine_messages().len(), 571);
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

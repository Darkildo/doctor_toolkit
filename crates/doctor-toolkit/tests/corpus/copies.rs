//! Census of the copies of resources: the same resource (type and ids) in
//! several streams (`docs/analysis/resource-copies.md`). Reads the game only.
//!
//! The table is printed; with `DOCTOR_COPIES_REPORT=<file>` it is also written
//! to the file.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Write as _;

use rayon::prelude::*;
use sha2::{Digest, Sha256};

use doctor_toolkit::container::ustream::ResourceHeader;
use doctor_toolkit::game::{self, GameInstance, StreamFile};
use doctor_toolkit::resource::names::type_tags;

use super::game_dir;

struct Entry {
    stream: usize,
    header: ResourceHeader,
    stored: u64,
    digest: [u8; 32],
}

#[derive(Default)]
struct TypeStats {
    resources: usize,
    keys: usize,
    keys_with_copies: usize,
    extra_copies: usize,
    max_copies: usize,
    keys_differing: usize,
    extra_bytes: u64,
    extra_stored: u64,
    copies_under_ancestor: usize,
}

impl TypeStats {
    fn add(&mut self, other: &TypeStats) {
        self.resources += other.resources;
        self.keys += other.keys;
        self.keys_with_copies += other.keys_with_copies;
        self.extra_copies += other.extra_copies;
        self.max_copies = self.max_copies.max(other.max_copies);
        self.keys_differing += other.keys_differing;
        self.extra_bytes += other.extra_bytes;
        self.extra_stored += other.extra_stored;
        self.copies_under_ancestor += other.copies_under_ancestor;
    }
}

/// The level a stream belongs to: `gf2/miami/local/...` and
/// `gf2/miami_global` → `miami`, `mp_bonnie/...` → `mp_bonnie`;
/// `gf2/globals_global` → `globals`.
fn level_of(stream: &str) -> &str {
    let mut parts = stream.split('/');
    let first = parts.next().unwrap_or_default();
    let level = if first == "gf2" {
        parts.next().unwrap_or_default()
    } else {
        first
    };
    level.strip_suffix("_global").unwrap_or(level)
}

/// Every resource of the game with a non-zero key lies in each of its
/// streams with the same header; copies within one level hold the same
/// data; the game's key (the ids, without the type) never names two types;
/// outside `gf2/globals_global` no stream holds a copy that one of its
/// prerequisites already holds.
#[test]
fn copy_census() {
    let Some(dir) = game_dir() else { return };
    let game = GameInstance::open(&dir).unwrap();
    let names: Vec<String> = game.streams().iter().map(|s| s.stream_name()).collect();
    let entries: Vec<Entry> = game
        .streams()
        .par_iter()
        .enumerate()
        .flat_map_iter(|(stream, file)| {
            let file = StreamFile::open(&file.path).unwrap();
            let layout = file.layout();
            (0..layout.resources.len())
                .map(|index| Entry {
                    stream,
                    header: layout.resources[index].header,
                    stored: layout.stored_size(index) as u64,
                    digest: Sha256::digest(layout.resource_data(file.data(), index).unwrap())
                        .into(),
                })
                .collect::<Vec<_>>()
        })
        .collect();
    assert_eq!(entries.len(), 212_544);

    let parents = game.stream_parents().unwrap();
    let by_name: HashMap<&str, usize> = names
        .iter()
        .enumerate()
        .map(|(i, n)| (n.as_str(), i))
        .collect();
    let ancestors: Vec<HashSet<usize>> = names
        .iter()
        .map(|n| {
            game::ancestors(&parents, n)
                .iter()
                .filter_map(|a| by_name.get(a.as_str()).copied())
                .collect()
        })
        .collect();

    let tags = type_tags();
    let tag = |id: u32| {
        tags.get(id)
            .map_or_else(|| format!("{id:08x}"), str::to_string)
    };
    let key_text = |ids: &[u32; 4]| {
        format!(
            "{:08x}.{:08x}.{:08x}.{:08x}",
            ids[0], ids[1], ids[2], ids[3]
        )
    };

    // Zero ids: one resource of the type per stream (`simgroup`,
    // `covernodetable`, ...); the loader does not register them.
    let mut zero_ids: BTreeMap<String, usize> = BTreeMap::new();
    let mut by_key: HashMap<(u32, [u32; 4]), Vec<usize>> = HashMap::new();
    for (i, e) in entries.iter().enumerate() {
        if e.header.ids == [0; 4] {
            *zero_ids.entry(tag(e.header.type_id)).or_default() += 1;
        } else {
            by_key
                .entry((e.header.type_id, e.header.ids))
                .or_default()
                .push(i);
        }
    }

    let mut failures = Vec::new();
    let mut stats: BTreeMap<String, TypeStats> = BTreeMap::new();
    let mut differing = Vec::new();
    let mut under_ancestor: BTreeMap<(String, String), usize> = BTreeMap::new();
    for ((type_id, ids), list) in &by_key {
        let t = tag(*type_id);
        let s = stats.entry(t.clone()).or_default();
        s.resources += list.len();
        s.keys += 1;
        if list.len() < 2 {
            continue;
        }
        s.keys_with_copies += 1;
        s.extra_copies += list.len() - 1;
        s.max_copies = s.max_copies.max(list.len());
        for &i in &list[1..] {
            s.extra_bytes += u64::from(entries[i].header.data_size);
            s.extra_stored += entries[i].stored;
        }
        let first = &entries[list[0]].header;
        if list.iter().any(|&i| {
            let h = &entries[i].header;
            (h.mem_policy, h.alignment) != (first.mem_policy, first.alignment)
        }) {
            failures.push(format!(
                "{t} {}: copies differ in memory policy or alignment",
                key_text(ids)
            ));
        }
        let variants: HashSet<[u8; 32]> = list.iter().map(|&i| entries[i].digest).collect();
        if variants.len() > 1 {
            s.keys_differing += 1;
            let mut per_level: BTreeMap<&str, HashSet<[u8; 32]>> = BTreeMap::new();
            for &i in list {
                per_level
                    .entry(level_of(&names[entries[i].stream]))
                    .or_default()
                    .insert(entries[i].digest);
            }
            if per_level.values().any(|v| v.len() > 1) {
                failures.push(format!(
                    "{t} {}: copies differ within one level",
                    key_text(ids)
                ));
            }
            let levels: Vec<String> = per_level.keys().map(|l| l.to_string()).collect();
            differing.push(format!(
                "{t} {} ×{}: {}",
                key_text(ids),
                list.len(),
                levels.join(" / ")
            ));
        }
        let streams: HashSet<usize> = list.iter().map(|&i| entries[i].stream).collect();
        for &i in list {
            let stream = entries[i].stream;
            if let Some(&a) = ancestors[stream].iter().find(|a| streams.contains(a)) {
                s.copies_under_ancestor += 1;
                *under_ancestor
                    .entry((names[a].clone(), t.clone()))
                    .or_default() += 1;
                if names[a] != "gf2/globals_global" {
                    failures.push(format!(
                        "{t} {}: {} holds a copy its prerequisite {} holds",
                        key_text(ids),
                        names[stream],
                        names[a]
                    ));
                }
            }
        }
    }

    // Textures with the same data under different keys: each takes its own
    // entry of the game's texture table (4096 entries, `exe/texture-table.md`).
    let tg4d = Some(doctor_toolkit::hash::string_id("tg4d"));
    let mut texture_keys: HashMap<[u8; 32], HashSet<[u32; 4]>> = HashMap::new();
    for e in entries.iter().filter(|e| Some(e.header.type_id) == tg4d) {
        texture_keys
            .entry(e.digest)
            .or_default()
            .insert(e.header.ids);
    }
    let texture_contents = texture_keys.len();
    let shared_contents: Vec<usize> = texture_keys
        .values()
        .map(HashSet::len)
        .filter(|&n| n > 1)
        .collect();
    let texture_key_count: usize = texture_keys.values().map(HashSet::len).sum();

    // The game's key: the ids only (the loader does not compare the type).
    let mut by_ids: HashMap<[u32; 4], HashSet<u32>> = HashMap::new();
    for e in entries.iter().filter(|e| e.header.ids != [0; 4]) {
        by_ids
            .entry(e.header.ids)
            .or_default()
            .insert(e.header.type_id);
    }
    for (ids, types) in by_ids.iter().filter(|(_, t)| t.len() > 1) {
        let list: Vec<String> = types.iter().map(|&t| tag(t)).collect();
        failures.push(format!(
            "{}: ids of several types {}",
            key_text(ids),
            list.join(", ")
        ));
    }

    let mut total = TypeStats::default();
    for s in stats.values() {
        total.add(s);
    }
    let all_size: u64 = entries.iter().map(|e| u64::from(e.header.data_size)).sum();
    let all_stored: u64 = entries.iter().map(|e| e.stored).sum();

    let mut out = String::new();
    writeln!(out, "type | resources | keys | with copies | extra copies | max | differing | extra MB (stored) | under prerequisite").unwrap();
    let row = |out: &mut String, t: &str, s: &TypeStats| {
        writeln!(
            out,
            "{t} | {} | {} | {} | {} | {} | {} | {:.1} ({:.1}) | {}",
            s.resources,
            s.keys,
            s.keys_with_copies,
            s.extra_copies,
            s.max_copies,
            s.keys_differing,
            s.extra_bytes as f64 / 1e6,
            s.extra_stored as f64 / 1e6,
            s.copies_under_ancestor
        )
        .unwrap();
    };
    for (t, s) in stats.iter().filter(|(_, s)| s.keys_with_copies > 0) {
        row(&mut out, t, s);
    }
    row(&mut out, "total", &total);
    writeln!(
        out,
        "\nall resources: {:.1} MB ({:.1} MB stored); one resource per stream (zero ids): {zero_ids:?}",
        all_size as f64 / 1e6,
        all_stored as f64 / 1e6
    )
    .unwrap();
    writeln!(out, "\ncopies a prerequisite holds: {under_ancestor:?}").unwrap();
    writeln!(
        out,
        "\ntextures: {texture_key_count} keys, {texture_contents} different data; {} data under several keys ({} keys, {} more than needed)",
        shared_contents.len(),
        shared_contents.iter().sum::<usize>(),
        shared_contents.iter().map(|n| n - 1).sum::<usize>()
    )
    .unwrap();
    differing.sort();
    writeln!(out, "\ncopies with different data (levels):").unwrap();
    for d in &differing {
        writeln!(out, "{d}").unwrap();
    }
    if let Some(path) = std::env::var_os("DOCTOR_COPIES_REPORT") {
        std::fs::write(path, &out).unwrap();
    }
    println!("{out}");

    assert!(failures.is_empty(), "{}", failures.join("\n"));
    assert_eq!(
        (
            total.keys_with_copies,
            total.extra_copies,
            total.keys_differing
        ),
        (13_020, 164_550, 14)
    );
    assert_eq!(stats["mesh"].keys_with_copies, 1_315);
}

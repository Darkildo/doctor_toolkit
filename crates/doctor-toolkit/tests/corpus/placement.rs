//! Placement of resources into streams: who uses each resource, where its
//! copies are, and how far the rule of `navigation::placement` explains them
//! (`docs/analysis/resource-placement.md`). Reads the game only.
//!
//! The table is printed; with `DOCTOR_PLACEMENT_REPORT=<file>` the users
//! that reach no copy and the copies the rule does not explain are also
//! written to the file.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use doctor_toolkit::game::GameInstance;
use doctor_toolkit::navigation::placement::Placement;
use doctor_toolkit::navigation::{GameIndex, RecordId};
use doctor_toolkit::resource::names::type_tags;
use doctor_toolkit::workspace::ResourceKey;

use super::game_dir;

/// One type: its users by how they reach a copy, its copies by why they
/// are where they are.
#[derive(Default, Debug, Clone, Copy, PartialEq, Eq)]
struct Row {
    /// Resources something refers to, and those whose copies are exactly
    /// where `place` puts them.
    keys: usize,
    exact: usize,
    /// Streams `place` puts a copy in, and those that hold one.
    predicted: usize,
    predicted_held: usize,
    /// (resource, user stream) pairs by where the user finds the resource:
    /// in itself, in a prerequisite, in a stream its data loads, nowhere.
    uses: usize,
    in_self: usize,
    in_prerequisite: usize,
    in_requested: usize,
    unreached: usize,
    /// Copies by their stream: a user, a prerequisite of users (shared), a
    /// stream a user loads by data, none of these.
    copies: usize,
    copy_in_user: usize,
    copy_shared: usize,
    copy_requested: usize,
    copy_other: usize,
    /// Copies of resources nothing refers to.
    unreferenced: usize,
}

impl Row {
    fn add(&mut self, o: &Row) {
        self.keys += o.keys;
        self.exact += o.exact;
        self.predicted += o.predicted;
        self.predicted_held += o.predicted_held;
        self.uses += o.uses;
        self.in_self += o.in_self;
        self.in_prerequisite += o.in_prerequisite;
        self.in_requested += o.in_requested;
        self.unreached += o.unreached;
        self.copies += o.copies;
        self.copy_in_user += o.copy_in_user;
        self.copy_shared += o.copy_shared;
        self.copy_requested += o.copy_requested;
        self.copy_other += o.copy_other;
        self.unreferenced += o.unreferenced;
    }
}

/// The level a stream belongs to: `gf2/miami/local/...` and
/// `gf2/miami_global` → `miami`, `mp_bonnie/...` → `mp_bonnie`.
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

/// Where a shared copy sits (a prerequisite of users that is not a user).
fn shared_kind(stream: &str) -> &'static str {
    if stream == "gf2/globals_global" {
        "globals_global"
    } else if stream.ends_with("_global") {
        "level _global"
    } else if stream.starts_with("gf2/") && stream.ends_with("/local") {
        "level local"
    } else if stream.starts_with("gf2/globalfolder/vehicles/") {
        "vehicle_shared"
    } else if stream.contains("/objectives/") || stream.contains("/checkpoints/") {
        "cut-scene folder"
    } else {
        "other"
    }
}

/// Every stream that refers to a resource reaches a copy: in itself, in a
/// prerequisite or in a stream its data loads, except the few listed in the
/// document; every copy of a referenced resource is in a stream that uses
/// it or in a prerequisite of such streams, except copies made for streams
/// that load the holder by data and a few listed ones.
#[test]
fn placement_census() {
    let Some(dir) = game_dir() else { return };
    let game = GameInstance::open(&dir).unwrap();
    let index = GameIndex::build(&game, &|_, _| {});
    let parents = game.stream_parents().unwrap();
    let placement = Placement::new(&index, &parents);
    let name = |s: usize| index.stream_name(s);
    let tags = type_tags();
    let record_of = |type_id: u32, ids: [u32; 4], stream: usize| -> RecordId {
        index
            .find_key(&ResourceKey {
                stream: name(stream).to_owned(),
                type_id,
                ids,
            })
            .expect("a holder holds the key")
    };

    let mut rows: BTreeMap<String, Row> = BTreeMap::new();
    let mut shared: BTreeMap<(&str, String), (usize, usize)> = BTreeMap::new();
    let mut unreached: BTreeMap<(&str, String), usize> = BTreeMap::new();
    let mut report = String::new();
    let mut keys: Vec<_> = placement.keys().collect();
    keys.sort();
    for (&(type_id, ids), holders) in keys {
        let tag = tags.display(type_id);
        let row = rows.entry(tag.clone()).or_default();
        let users = placement.users(type_id, ids);
        if users.is_empty() {
            row.unreferenced += holders.len();
            continue;
        }
        row.keys += 1;
        let predicted = placement.place(users);
        row.exact += usize::from(predicted == holders);
        row.predicted += predicted.len();
        row.predicted_held += predicted.iter().filter(|p| holders.contains(p)).count();
        let label = || index.label(record_of(type_id, ids, holders[0]));
        for &u in users {
            row.uses += 1;
            if holders.contains(&u) {
                row.in_self += 1;
            } else if holders.iter().any(|h| placement.ancestors(u).contains(h)) {
                row.in_prerequisite += 1;
            } else if holders.iter().any(|&h| placement.reaches(u, h)) {
                row.in_requested += 1;
            } else {
                row.unreached += 1;
                let level = level_of(name(u));
                let class = if holders.iter().any(|&h| level_of(name(h)) == level) {
                    "a copy in another stream of the level"
                } else if name(u).starts_with("mp_") {
                    "multiplayer map, copies only in other levels"
                } else {
                    "copies only in other levels"
                };
                *unreached.entry((class, tag.clone())).or_default() += 1;
                writeln!(
                    report,
                    "unreached {tag} [{}] used in {}; held by {}: {:?}",
                    label(),
                    name(u),
                    holders.len(),
                    holders.iter().take(3).map(|&h| name(h)).collect::<Vec<_>>()
                )
                .unwrap();
            }
        }
        for &h in holders {
            row.copies += 1;
            let served = users
                .iter()
                .filter(|&&u| placement.ancestors(u).contains(&h))
                .count();
            if users.contains(&h) {
                row.copy_in_user += 1;
            } else if served > 0 {
                row.copy_shared += 1;
                let e = shared
                    .entry((shared_kind(name(h)), tag.clone()))
                    .or_default();
                e.0 += 1;
                e.1 += served;
            } else if users.iter().any(|&u| placement.reaches(u, h)) {
                row.copy_requested += 1;
            } else {
                row.copy_other += 1;
                writeln!(
                    report,
                    "extra {tag} [{}] in {}; used in {}: {:?}",
                    label(),
                    name(h),
                    users.len(),
                    users.iter().take(3).map(|&u| name(u)).collect::<Vec<_>>()
                )
                .unwrap();
            }
        }
    }

    let mut total = Row::default();
    for r in rows.values() {
        total.add(r);
    }
    let pct = |a: usize, b: usize| {
        if b == 0 {
            "-".to_owned()
        } else {
            format!("{:.2}", a as f64 * 100.0 / b as f64)
        }
    };
    let mut out = String::new();
    writeln!(
        out,
        "type | keys used | placed exactly | place: precision % | recall % | uses | in self | in prerequisite | in requested | unreached | reached % \
         || copies | in user | shared | requested | other | needed % | unreferenced"
    )
    .unwrap();
    let mut line = |t: &str, r: &Row| {
        writeln!(
            out,
            "{t} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} || {} | {} | {} | {} | {} | {} | {}",
            r.keys,
            r.exact,
            pct(r.predicted_held, r.copies),
            pct(r.predicted_held, r.predicted),
            r.uses,
            r.in_self,
            r.in_prerequisite,
            r.in_requested,
            r.unreached,
            pct(r.uses - r.unreached, r.uses),
            r.copies,
            r.copy_in_user,
            r.copy_shared,
            r.copy_requested,
            r.copy_other,
            pct(r.copies - r.copy_other, r.copies),
            r.unreferenced
        )
        .unwrap();
    };
    for (t, r) in rows.iter().filter(|(_, r)| r.keys + r.unreferenced > 0) {
        line(t, r);
    }
    line("total", &total);
    writeln!(out, "\nshared copies (where, type): copies, users served").unwrap();
    for ((kind, t), (copies, served)) in &shared {
        writeln!(out, "{kind} {t}: {copies}, {served}").unwrap();
    }
    writeln!(out, "\nunreached uses (why, type): uses").unwrap();
    for ((class, t), count) in &unreached {
        writeln!(out, "{class} {t}: {count}").unwrap();
    }
    println!("{out}");
    if let Some(path) = std::env::var_os("DOCTOR_PLACEMENT_REPORT") {
        std::fs::write(path, format!("{out}\n{report}")).unwrap();
    }

    // `place` (every user, except one under another user) gives exactly the
    // streams of 96.5 % of the referenced resources; the rest are shared
    // in a common prerequisite or found in a stream loaded by data.
    assert_eq!((total.keys, total.exact), (40_065, 38_672));
    assert_eq!((total.predicted, total.predicted_held), (262_119, 201_626));
    // Users: 99.76 % reach a copy.
    assert_eq!(
        (
            total.uses,
            total.in_self,
            total.in_prerequisite,
            total.in_requested,
            total.unreached
        ),
        (292_145, 201_727, 86_183, 3_528, 707)
    );
    assert_eq!(
        unreached,
        BTreeMap::from([
            (("a copy in another stream of the level", "graph".into()), 1),
            (
                ("a copy in another stream of the level", "material".into()),
                414
            ),
            (("copies only in other levels", "material".into()), 48),
            (
                (
                    "multiplayer map, copies only in other levels",
                    "material".into()
                ),
                244
            ),
        ])
    );
    // Copies: 99.96 % are in a user, shared above users or in a stream a
    // user loads by data.
    assert_eq!(
        (
            total.copies,
            total.copy_in_user,
            total.copy_shared,
            total.copy_requested,
            total.copy_other,
            total.unreferenced
        ),
        (202_909, 201_727, 941, 164, 77, 6_883)
    );
    let other: BTreeMap<&str, usize> = rows
        .iter()
        .filter(|(_, r)| r.copy_other > 0)
        .map(|(t, r)| (t.as_str(), r.copy_other))
        .collect();
    assert_eq!(
        other,
        BTreeMap::from([("bnk", 11), ("material", 6), ("tg4h", 13), ("vfx", 47)])
    );
    // Every swap list sits in the stream of the object with its GUID.
    assert_eq!(rows["materialswaplistresource"].exact, 188);
    assert_eq!(rows["materialswaplistresource"].unreferenced, 0);
}

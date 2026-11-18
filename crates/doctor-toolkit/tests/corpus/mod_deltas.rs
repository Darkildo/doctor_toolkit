//! Stage 110: mods of format version 6 on the files of the game.
//!
//! The mod build stores a resource as a delta when the delta is exact and
//! smaller than the resource; the mod manager folds the deltas of the
//! enabled mods. The first test counts how often the rule of the build
//! gives a delta on the SimGroups of the game; the second makes two mods of
//! random edits of the same zones and writes both into copies of them.
//! The files of the game are only read.

use std::collections::BTreeMap;
use std::sync::Mutex;

use doctor_toolkit::mods::{self, KeptWhole};
use doctor_toolkit::resource::delta::Kind;
use doctor_toolkit::resource::simgroup::SimGroup;

use super::check_every;
use super::delta::{Author, play};

/// What an author does to a group, for the count of the build's rule.
type Script = (&'static str, fn(&mut Author) -> Result<(), String>);

const SCRIPTS: &[Script] = &[
    ("set one attribute", |a| a.set_one().map(drop)),
    ("one new object, moved", |a| match a.duplicate_one()? {
        Some(guid) => a.move_again(guid).or(Ok(())),
        None => Ok(()),
    }),
    ("three new objects", |a| {
        for _ in 0..3 {
            a.duplicate_one()?;
        }
        Ok(())
    }),
    ("six mixed edits", |a| a.mixed(6)),
    ("twelve mixed edits", |a| a.mixed(12)),
];

#[derive(Debug, Default, Clone, Copy)]
struct Share {
    cases: usize,
    deltas: usize,
    not_smaller: usize,
    other: usize,
    delta_bytes: u64,
    resource_bytes: u64,
}

/// The rule of the build ([`mods::delta_for`]) on every SimGroup of the
/// game: how many resources go as deltas for edits of several sizes. An
/// edit of an attribute is a delta nearly always; new objects are one
/// while the group is larger than what is added to it.
#[test]
fn the_rule_of_the_build_on_every_simgroup() {
    let shares: Mutex<BTreeMap<usize, Share>> = Mutex::new(BTreeMap::new());
    let type_id = Kind::SimGroup.type_id();
    let count = check_every("simgroup", |data| {
        let donor = SimGroup::parse(data).map_err(|e| e.to_string())?;
        if donor.objects.is_empty() {
            return Ok(());
        }
        for (number, (name, script)) in SCRIPTS.iter().enumerate() {
            let seed = 0xA076_1D64_78BD_642F ^ (data.len() as u64) << 8 ^ number as u64;
            let mut author = Author::new(&donor, seed);
            script(&mut author).map_err(|e| format!("{name}: {e}"))?;
            if author.edits.is_empty() {
                continue;
            }
            let mine =
                play(data, &donor, &author.edits, true).map_err(|e| format!("{name}: {e}"))?;
            if mine == data {
                continue;
            }
            let mut share = Share {
                cases: 1,
                ..Share::default()
            };
            match mods::delta_for(type_id, data, &mine) {
                Ok(document) => {
                    share.deltas = 1;
                    share.delta_bytes = document.len() as u64;
                    share.resource_bytes = mine.len() as u64;
                }
                Err(KeptWhole::NotSmaller { .. }) => share.not_smaller = 1,
                Err(_) => share.other = 1,
            }
            let mut shares = shares.lock().unwrap();
            let total = shares.entry(number).or_default();
            total.cases += share.cases;
            total.deltas += share.deltas;
            total.not_smaller += share.not_smaller;
            total.other += share.other;
            total.delta_bytes += share.delta_bytes;
            total.resource_bytes += share.resource_bytes;
        }
        Ok(())
    });
    if count == 0 {
        return;
    }
    assert_eq!(count, 2006);
    let shares = shares.into_inner().unwrap();
    eprintln!("The rule of the mod build on {count} SimGroups:");
    for (number, share) in &shares {
        let deltas = share.deltas.max(1) as u64;
        eprintln!(
            "  {:<24} cases {:>4}  deltas {:>4} ({:5.1}%)  not smaller {:>4}  not operations \
             {:>2}  delta {:>5} B of {:>6} B",
            SCRIPTS[*number].0,
            share.cases,
            share.deltas,
            100.0 * share.deltas as f64 / share.cases as f64,
            share.not_smaller,
            share.other,
            share.delta_bytes / deltas,
            share.resource_bytes / deltas,
        );
    }
    for (number, share) in &shares {
        let name = SCRIPTS[*number].0;
        // Stage 109: every edit is an exact delta; what stays whole stays
        // so for its size alone.
        assert_eq!(share.other, 0, "{name}");
        assert_eq!(share.deltas + share.not_smaller, share.cases, "{name}");
    }
    // An attribute set in a group is smaller than the group.
    let attribute = shares[&0];
    assert!(
        attribute.deltas * 100 >= attribute.cases * 95,
        "{attribute:?}"
    );
}

#[cfg(unix)]
pub(super) mod folded {
    use std::collections::HashSet;
    use std::time::Instant;

    use doctor_toolkit::game::{GameInstance, StreamFile};
    use doctor_toolkit::mods::manager::{ModApplyOptions, Problem};
    use doctor_toolkit::mods::{self, ModArchive, ModInfo, ModManager};
    use doctor_toolkit::resource::delta::Kind;
    use doctor_toolkit::resource::simgroup::{Guid, SimGroup};
    use doctor_toolkit::workspace::{ResourceEdit, ResourceKey, StorageChoice, Workspace};

    use super::super::delta::{Author, Edit, play};
    use super::super::{game_dir, shadow_game, simgroup_view};

    /// A zone of the test: its stream, the SimGroup in it and its data.
    pub(crate) struct Zone {
        pub(crate) stream: String,
        pub(crate) rel_path: String,
        pub(crate) index: usize,
        pub(crate) data: Vec<u8>,
    }

    /// The first `count` streams of the game, by name, that hold one
    /// SimGroup of at least eight objects in a file of at most 8 MiB.
    pub(crate) fn zones(game: &GameInstance, count: usize) -> Vec<Zone> {
        let type_id = Kind::SimGroup.type_id();
        let mut files: Vec<_> = game.streams().iter().collect();
        files.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
        let mut out = Vec::new();
        for file in files {
            if out.len() == count {
                break;
            }
            if file.len > 8 << 20 {
                continue;
            }
            let stream = StreamFile::open(&file.path).unwrap();
            let groups: Vec<usize> = (0..stream.layout().resources.len())
                .filter(|i| stream.layout().resources[*i].header.type_id == type_id)
                .collect();
            let [index] = groups[..] else { continue };
            let data = stream.resource_data(index).unwrap();
            if SimGroup::parse(&data).is_ok_and(|g| g.objects.len() >= 8) {
                out.push(Zone {
                    stream: file.stream_name(),
                    rel_path: file.rel_path.clone(),
                    index,
                    data,
                });
            }
        }
        out
    }

    /// What one author did to a zone.
    #[derive(Default)]
    struct Done {
        added: Vec<Guid>,
        removed: HashSet<Guid>,
        touched: HashSet<Guid>,
    }

    /// The mod `id` of an author who makes four mixed edits of every zone
    /// and then adds two objects to it. Returns the file and, for each
    /// zone, what was done.
    fn author_mod(
        game: &GameInstance,
        zones: &[Zone],
        temp: &std::path::Path,
        id: &'static str,
        seed: u64,
    ) -> (std::path::PathBuf, mods::BuildReport, Vec<Done>) {
        let mut ws = Workspace::create(temp.join(format!("ws-{id}")), game).unwrap();
        let mut done = Vec::new();
        for zone in zones {
            let donor = SimGroup::parse(&zone.data).unwrap();
            let mut author = Author::named(&donor, seed ^ zone.data.len() as u64, id);
            author.mixed(4).unwrap();
            author.duplicate_one().unwrap();
            author.copy_one().unwrap();
            let mine = play(&zone.data, &donor, &author.edits, true).unwrap();
            let mut what = Done::default();
            for edit in &author.edits {
                what.touched.extend(edit.touches());
                match edit {
                    Edit::Duplicate(_, guid, _) | Edit::Copy(_, guid, _) => what.added.push(*guid),
                    Edit::Remove(guid) => {
                        what.removed.insert(*guid);
                    }
                    _ => {}
                }
            }
            // An object the author added and then removed is not added.
            what.added.retain(|guid| !what.removed.contains(guid));
            done.push(what);
            let edit = ResourceEdit {
                index: zone.index,
                data: mine,
                storage: StorageChoice::Keep,
            };
            let stream = game.find_stream(&zone.stream).unwrap();
            ws.set_resources(game, "edit", stream, &[edit]).unwrap();
        }
        let out = temp.join(format!("{id}.zip"));
        let info = ModInfo {
            id: id.to_owned(),
            name: format!("Author {id}"),
            version: "1.0".into(),
            ..ModInfo::default()
        };
        let report = mods::build_from_workspace(&ws, &info, None, None, &out).unwrap();
        (out, report, done)
    }

    /// Two mods made of random edits of the same SimGroups of real zones,
    /// each on the game's own data: installed together and written into
    /// copies of the zones, every stream parses and its group holds the
    /// objects both mods added; objects neither mod touched are as they
    /// were. Restoring the originals gives the files of the game back.
    /// `DOCTOR_FOLD_ZONES` sets the number of zones (24 by default).
    #[test]
    fn two_mods_of_random_edits_fold_on_copies_of_real_zones() {
        let Some(dir) = game_dir() else { return };
        let count = std::env::var("DOCTOR_FOLD_ZONES")
            .ok()
            .and_then(|n| n.parse().ok())
            .unwrap_or(24);
        let real = GameInstance::open(&dir).unwrap();
        let zones = zones(&real, count);
        assert_eq!(zones.len(), count, "zones with a SimGroup of eight objects");
        // Links to the zones and to every table of contents: a write
        // replaces a link, never a file of the game.
        let mut files: Vec<&str> = zones.iter().map(|z| z.rel_path.as_str()).collect();
        files.extend(real.tocs().iter().map(|t| t.rel_path.as_str()));
        files.push("godfather2.exe");
        let shadow = shadow_game(&dir, &files);
        let game = GameInstance::open(shadow.path()).unwrap();
        let temp = tempfile::tempdir().unwrap();

        let (a, built_a, done_a) = author_mod(&game, &zones, temp.path(), "alpha", 0x51ED_270B);
        let (b, built_b, done_b) = author_mod(&game, &zones, temp.path(), "beta", 0x2545_F491);
        let delta_keys = |path: &std::path::Path| -> HashSet<ResourceKey> {
            let archive = ModArchive::open(path).unwrap();
            archive.manifest().deltas().map(|r| r.key.clone()).collect()
        };
        let (deltas_a, deltas_b) = (delta_keys(&a), delta_keys(&b));

        let mut mods = ModManager::open(&game).unwrap();
        for path in [&a, &b] {
            let report = mods.install_file(&game, path, true).unwrap();
            assert!(
                report.issues.iter().all(|i| !i.is_fatal()),
                "{:?}",
                report.issues
            );
        }
        // A manager that has computed nothing yet: every delta is applied.
        let cold = ModManager::open(&game).unwrap();
        let started = Instant::now();
        let plan = cold.plan(&game).unwrap();
        let folding = started.elapsed();
        assert_eq!(plan.len(), count);
        let status = mods.status(&game).unwrap();
        assert!(status.streams.iter().all(|s| !s.up_to_date));
        let issues = mods.check(&game).unwrap();
        // The authors worked apart: an object one removed and the other
        // changed is an operation left out, which is said and stops nothing.
        let skipped = issues
            .iter()
            .filter(|i| matches!(i.problem, Problem::OperationsSkipped { .. }))
            .count();
        assert!(
            issues.iter().all(|i| i.problem.is_notice()),
            "{:?}",
            issues.iter().find(|i| !i.problem.is_notice())
        );
        let conflicts = mods.conflicts(&game, true).unwrap();
        let started = Instant::now();
        mods.apply(&game, ModApplyOptions::default()).unwrap();
        let writing = started.elapsed();
        // A manager opened after the write knows every step from the index.
        let warm = ModManager::open(&game).unwrap();
        let started = Instant::now();
        assert_eq!(warm.plan(&game).unwrap(), plan);
        let known = started.elapsed();
        assert!(
            warm.status(&game)
                .unwrap()
                .streams
                .iter()
                .all(|s| s.up_to_date)
        );

        let mut merged = 0;
        for (n, zone) in zones.iter().enumerate() {
            let written = StreamFile::open(shadow.path().join(&zone.rel_path)).unwrap();
            let data = written.resource_data(zone.index).unwrap();
            let group = SimGroup::parse(&data).unwrap();
            assert_eq!(group.encode(), data, "{}", zone.stream);
            let has = |guid: &Guid| group.objects.iter().any(|o| o.guid == *guid);
            let key =
                ResourceKey::new(&zone.stream, &written.layout().resources[zone.index].header);
            // The second mod is in whole or as a delta; the first is under
            // it unless the second replaces the resource.
            assert!(done_b[n].added.iter().all(has), "{}: beta", zone.stream);
            if !deltas_b.contains(&key) {
                continue;
            }
            merged += 1;
            assert!(done_a[n].added.iter().all(has), "{}: alpha", zone.stream);
            assert!(!done_a[n].added.is_empty() && !done_b[n].added.is_empty());
            // What neither touched is what the game has.
            let original = SimGroup::parse(&zone.data).unwrap();
            let view = |group: &SimGroup, guid: &Guid| {
                let at = group.objects.iter().position(|o| o.guid == *guid)?;
                Some(simgroup_view(group).swap_remove(at))
            };
            for object in &original.objects {
                let guid = &object.guid;
                if done_a[n].touched.contains(guid) || done_b[n].touched.contains(guid) {
                    continue;
                }
                assert!(
                    view(&group, guid) == view(&original, guid),
                    "{}: object {guid}",
                    zone.stream
                );
            }
        }
        eprintln!(
            "Two mods on {count} zones: deltas {} and {} of {count}; merged in {merged} zones, \
             {skipped} resource(s) with operations left out, {} conflict(s) by element; \
             folding {folding:.2?}, the same from the index {known:.2?}, writing the game \
             {writing:.2?}",
            built_a.deltas,
            built_b.deltas,
            conflicts.iter().filter(|c| !c.elements.is_empty()).count(),
        );
        assert_eq!(deltas_a.len(), built_a.deltas);
        // Groups of eight objects and more: most edits are smaller than
        // the group.
        assert!(merged * 2 >= count, "{merged} of {count} zones merged");

        // The originals come back byte for byte, and the game was not
        // written: its files are what the links point to.
        mods.unapply(&game, false).unwrap();
        for rel in &files {
            let path = shadow.path().join(rel);
            assert!(
                std::fs::read(&path).unwrap() == std::fs::read(dir.join(rel)).unwrap(),
                "{rel}"
            );
        }
    }
}

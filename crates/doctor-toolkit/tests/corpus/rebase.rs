//! Stage 111: rebasing on the files of the game.
//!
//! An edit made on the data a mod gave is moved onto the data the mod
//! gives after its update: the operations of the edit, taken by the
//! contents of the group and not by its layout, applied to the new data.
//! The first test does that for random edits of every SimGroup of the game
//! and compares the result with the same edits made by hand on the changed
//! group; the second does it through a workspace on copies of real zones,
//! where the "mod" is a write into the zone that leaves a backup, as the
//! mod manager's does. The files of the game are only read.

use std::collections::HashSet;
use std::sync::atomic::{AtomicUsize, Ordering};

use doctor_toolkit::resource::delta::{self, Kind};
use doctor_toolkit::resource::simgroup::{Guid, SimGroup};
use doctor_toolkit::resource::simgroup_edit::new_object_guid;

use super::delta::{Author, Edit, play};
use super::{check_every, simgroup_view};

fn find(group: &SimGroup, guid: &Guid) -> Option<usize> {
    group.objects.iter().position(|o| o.guid == *guid)
}

/// What the update of a mod does to a group under the edits of an author,
/// who touched the objects `touched`: it adds an object of its own, and
/// copies, removes and moves objects the author did not touch, as many of
/// the three as the group has. `name` makes the GUIDs of its new objects.
fn update_of_the_mod(donor: &SimGroup, touched: &HashSet<Guid>, name: &str) -> Vec<Edit> {
    let free: Vec<Guid> = donor
        .objects
        .iter()
        .map(|o| o.guid)
        .filter(|g| !touched.contains(g))
        .collect();
    let taken = |g: &Guid| find(donor, g).is_some() || touched.contains(g);
    let mut edits = vec![Edit::Copy(
        0,
        new_object_guid(&format!("{name} copy"), taken),
        [9.0, 8.0, 7.0],
    )];
    if let Some(&source) = free.first() {
        let guid = new_object_guid(&format!("{name} duplicate"), taken);
        edits.push(Edit::Duplicate(source, guid, [0.0, 3.0, 0.0]));
    }
    if let Some(&victim) = free.get(1) {
        edits.push(Edit::Remove(victim));
    }
    if let Some(third) = free.get(2)
        && let Some(at) = find(donor, third)
        && donor.transform(&donor.objects[at]).is_some()
    {
        edits.push(Edit::Move(*third, [5.0, 6.0, 7.0]));
    }
    edits
}

/// Six mixed edits of every SimGroup, written as a project build writes
/// them — all on one parsed group, a layout no delta was checked on — are
/// rebased onto the group as another mod changed it: the operations
/// between the base and the edited group ([`delta::operations`]) on the
/// changed group give what the same edits give there by hand.
#[test]
fn simgroup_edits_are_rebased_onto_a_changed_base_as_the_edits_by_hand() {
    let done = AtomicUsize::new(0);
    let unchanged = AtomicUsize::new(0);
    let type_id = Kind::SimGroup.type_id();
    let count = check_every("simgroup", |data| {
        let donor = SimGroup::parse(data).map_err(|e| e.to_string())?;
        if donor.objects.is_empty() {
            unchanged.fetch_add(1, Ordering::Relaxed);
            return Ok(());
        }
        let mut author = Author::new(&donor, 0x9E37_79B9_7F4A_7C15 ^ data.len() as u64);
        author.mixed(6)?;
        let mine = play(data, &donor, &author.edits, false)?;
        let touched: HashSet<Guid> = author.edits.iter().flat_map(Edit::touches).collect();
        let changed = play(
            data,
            &donor,
            &update_of_the_mod(&donor, &touched, "other"),
            true,
        )?;
        let operations = delta::operations(type_id, data, &mine)
            .map_err(|e| e.to_string())?
            .ok_or("the edits are not operations")?;
        let applied = operations.apply(&changed).map_err(|e| e.to_string())?;
        if !applied.skipped.is_empty() || !applied.conflicts.is_empty() {
            return Err(format!(
                "skipped {:?}, conflicts {:?}",
                applied.skipped, applied.conflicts
            ));
        }
        let got = SimGroup::parse(&applied.data).map_err(|e| format!("the result: {e}"))?;
        let by_hand = play(&changed, &donor, &author.edits, true)?;
        let expected = SimGroup::parse(&by_hand).map_err(|e| e.to_string())?;
        if simgroup_view(&got) != simgroup_view(&expected) {
            return Err(format!(
                "the rebased edits differ from the edits by hand: {:?}",
                author.edits
            ));
        }
        if got.encode() != applied.data {
            return Err("the result is not written as it is read".into());
        }
        done.fetch_add(1, Ordering::Relaxed);
        Ok(())
    });
    if count == 0 {
        return;
    }
    let (done, unchanged) = (done.into_inner(), unchanged.into_inner());
    eprintln!(
        "SimGroup edits rebased onto a changed base as the edits by hand: {done} of {count} \
         groups ({unchanged} without objects)"
    );
    assert_eq!(done + unchanged, count);
}

#[cfg(unix)]
mod on_zones {
    use std::collections::HashSet;
    use std::time::Instant;

    use doctor_toolkit::container::ustream::{Replacement, Storage};
    use doctor_toolkit::game::GameInstance;
    use doctor_toolkit::resource::delta::Kind;
    use doctor_toolkit::resource::simgroup::{Guid, SimGroup};
    use doctor_toolkit::resource::simgroup_edit::new_object_guid;
    use doctor_toolkit::workspace::{
        Blob, Change, RebaseOutcome, ResourceEdit, ResourceKey, StorageChoice, Workspace,
    };

    use super::super::delta::{Author, Edit, play};
    use super::super::mod_deltas::folded::zones;
    use super::super::{game_dir, shadow_game, simgroup_view};
    use super::{find, update_of_the_mod};

    /// A workspace edits the SimGroups of real zones on the data a mod
    /// gave them; the mod is updated; every change is stale, one rebase
    /// moves them all, and each group is what the edits give by hand on
    /// the updated data. One undo gives the changes back as they were.
    /// `DOCTOR_REBASE_ZONES` sets the number of zones (12 by default).
    #[test]
    fn a_workspace_is_rebased_on_copies_of_real_zones() {
        let Some(dir) = game_dir() else { return };
        let count = std::env::var("DOCTOR_REBASE_ZONES")
            .ok()
            .and_then(|n| n.parse().ok())
            .unwrap_or(12);
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
        let game_gets = |stream: usize, index: usize, data: &[u8]| {
            let replacement = Replacement {
                index,
                data,
                storage: Storage::RefPack,
            };
            game.replace_resources(stream, &[replacement], Default::default())
                .unwrap();
        };
        let donors: Vec<SimGroup> = zones
            .iter()
            .map(|z| SimGroup::parse(&z.data).unwrap())
            .collect();
        let streams: Vec<usize> = zones
            .iter()
            .map(|z| game.find_stream(&z.stream).unwrap())
            .collect();

        // The mod, version 1: an object of its own in every zone.
        let mut first = Vec::new();
        for (n, zone) in zones.iter().enumerate() {
            let donor = &donors[n];
            let guid = new_object_guid("mod one", |g| find(donor, g).is_some());
            let data = play(
                &zone.data,
                donor,
                &[Edit::Copy(0, guid, [1.0, 2.0, 3.0])],
                true,
            )
            .unwrap();
            game_gets(streams[n], zone.index, &data);
            first.push((guid, data));
        }

        // The edits of the workspace, made on the game with the mod.
        let mut ws = Workspace::create(temp.path().join("ws"), &game).unwrap();
        let mut edits: Vec<Vec<Edit>> = Vec::new();
        let mut keys: Vec<ResourceKey> = Vec::new();
        for (n, zone) in zones.iter().enumerate() {
            let (_, with_mod) = &first[n];
            assert!(ws.current_data(&game, streams[n], zone.index).unwrap() == *with_mod);
            let mut author = Author::named(&donors[n], 0x2545_F491 ^ zone.data.len() as u64, "me");
            author.mixed(4).unwrap();
            author.duplicate_one().unwrap();
            let mine = play(with_mod, &donors[n], &author.edits, true).unwrap();
            let edit = ResourceEdit {
                index: zone.index,
                data: mine,
                storage: StorageChoice::Keep,
            };
            ws.set_resources(&game, "edit", streams[n], &[edit])
                .unwrap();
            edits.push(author.edits);
        }
        for change in ws.changes() {
            assert!(change.made_on.is_some(), "{}", change.key);
            assert_eq!(change.key.type_id, Kind::SimGroup.type_id());
            keys.push(change.key.clone());
        }
        assert_eq!(keys.len(), count);
        assert!(ws.stale(&game).unwrap().is_empty());
        let before: Vec<Change> = ws.changes().cloned().collect();

        // The mod, version 2: more objects, and objects of the game the
        // workspace did not touch are copied, removed and moved.
        let mut second = Vec::new();
        for (n, zone) in zones.iter().enumerate() {
            let (guid, with_mod) = &first[n];
            let mut touched: HashSet<Guid> = edits[n].iter().flat_map(Edit::touches).collect();
            touched.insert(*guid);
            let update = update_of_the_mod(&donors[n], &touched, "mod two");
            let data = play(with_mod, &donors[n], &update, true).unwrap();
            game_gets(streams[n], zone.index, &data);
            second.push(data);
        }

        let started = Instant::now();
        let stale = ws.stale(&game).unwrap();
        let finding = started.elapsed();
        assert_eq!(stale.len(), count);
        assert!(stale.iter().all(|s| s.kind == Some(Kind::SimGroup)));
        let head = ws.head();
        let started = Instant::now();
        let report = ws.rebase(&game, None).unwrap();
        let rebasing = started.elapsed();
        assert!(report.seq.is_some());
        assert_eq!(ws.head(), head + 1, "one operation");
        assert_eq!(report.changes.len(), count);
        let mut operations = 0;
        for rebased in &report.changes {
            let RebaseOutcome::Merged {
                applied,
                skipped,
                conflicts,
                removed,
            } = &rebased.outcome
            else {
                panic!("{}: {:?}", rebased.key, rebased.outcome);
            };
            assert!(
                skipped.is_empty() && conflicts.is_empty() && !removed,
                "{}: {:?}",
                rebased.key,
                rebased.outcome
            );
            operations += applied;
        }
        assert!(ws.stale(&game).unwrap().is_empty());
        for (n, zone) in zones.iter().enumerate() {
            let key = keys
                .iter()
                .find(|k| k.stream == zone.stream)
                .expect("a change of the zone");
            let change = ws.change(key).unwrap();
            assert_eq!(
                change.made_on,
                Some(Blob::of(&second[n])),
                "{}",
                zone.stream
            );
            let data = ws.data(key).unwrap().unwrap();
            let got = SimGroup::parse(&data).unwrap();
            assert!(got.encode() == data, "{}", zone.stream);
            let by_hand = play(&second[n], &donors[n], &edits[n], true).unwrap();
            let expected = SimGroup::parse(&by_hand).unwrap();
            assert!(
                simgroup_view(&got) == simgroup_view(&expected),
                "{}: the rebased group differs from the edits by hand",
                zone.stream
            );
        }
        eprintln!(
            "A workspace rebased on {count} zones: {operations} operation(s) applied, none \
             left out; finding the stale changes {finding:.2?}, rebasing {rebasing:.2?}"
        );

        // One undo: every change as it was, stale again.
        ws.undo().unwrap();
        assert!(ws.changes().cloned().collect::<Vec<_>>() == before);
        assert_eq!(ws.stale(&game).unwrap().len(), count);

        // The game was not written: the zones in the shadow are files of
        // their own now, and the links to the tables of contents are links.
        for zone in &zones {
            let meta = std::fs::symlink_metadata(shadow.path().join(&zone.rel_path)).unwrap();
            assert!(!meta.file_type().is_symlink(), "{}", zone.rel_path);
        }
    }
}

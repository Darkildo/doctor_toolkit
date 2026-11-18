//! Rebasing on the miniature game with its town
//! ([`crate::mods::fold::tests::town`]): what an edit records of the data
//! it is made on, which changes are stale when something else changes the
//! game under them, and what a rebase does with each kind. "Something
//! else" is a write into the files of the game that leaves a backup, as
//! the mod manager makes one; the same with real mods and a project is in
//! `project/rebase/tests.rs`.

use std::fs;
use std::path::Path;

use super::*;
use crate::container::ustream::{Replacement, Storage};
use crate::mods::fold::tests::{add_crate, edit_group, guid, has, object, town};
use crate::resource::simgroup::SimGroup;
use crate::testing::{LEVEL, MATERIAL, MESH, STRINGS, YARD, YARD_SIMGROUP};
use crate::workspace::tests::{edit, game_data, key, stream};

/// Something else than the workspace — a mod — gives resource `index` of
/// stream `name` the data `data`.
fn game_gets(game: &GameInstance, name: &str, index: usize, data: &[u8]) {
    let replacement = Replacement {
        index,
        data,
        storage: Storage::RefPack,
    };
    game.replace_resources(stream(game, name), &[replacement], Default::default())
        .unwrap();
}

fn set(ws: &mut Workspace, game: &GameInstance, name: &str, index: usize, data: Vec<u8>) {
    ws.set_resources(game, "edit", stream(game, name), &[edit(index, data)])
        .unwrap();
}

/// Removes every `made_on` from a JSON file of a workspace: the file as a
/// program before stage 111 wrote it.
fn forget_made_on(path: &Path) {
    fn strip(value: &mut serde_json::Value) {
        match value {
            serde_json::Value::Object(map) => {
                map.remove("made_on");
                map.values_mut().for_each(strip);
            }
            serde_json::Value::Array(list) => list.iter_mut().for_each(strip),
            _ => {}
        }
    }
    let mut value: serde_json::Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    strip(&mut value);
    fs::write(path, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
}

#[test]
fn an_edit_records_the_data_it_was_made_on_when_that_is_not_the_original() {
    let (_dir, game) = town();
    let temp = tempfile::tempdir().unwrap();
    let level = stream(&game, LEVEL);
    let mesh = key(&game, LEVEL, MESH);
    let original = game_data(&game, LEVEL, MESH);

    // On the unmodified game nothing is written down, and the files of the
    // workspace are what they were before the field existed.
    let mut plain = Workspace::create(temp.path().join("plain"), &game).unwrap();
    set(&mut plain, &game, LEVEL, MESH, vec![1; 10]);
    assert_eq!(plain.change(&mesh).unwrap().made_on, None);
    assert_eq!(*plain.change(&mesh).unwrap().made_on(), Blob::of(&original));
    for file in ["manifest.json", "log.json"] {
        let text = fs::read_to_string(plain.dir().join(file)).unwrap();
        assert!(!text.contains("made_on"), "{file}");
    }
    assert!(plain.stale(&game).unwrap().is_empty());
    drop(plain);

    // A mod gives the mesh other data: an edit starts from it and records it.
    game_gets(&game, LEVEL, MESH, &[2; 20]);
    let dir = temp.path().join("on-a-mod");
    let mut ws = Workspace::create(&dir, &game).unwrap();
    assert_eq!(ws.current_data(&game, level, MESH).unwrap(), [2; 20]);
    set(&mut ws, &game, LEVEL, MESH, vec![3; 30]);
    let first = ws.change(&mesh).unwrap().clone();
    assert_eq!(first.original, Blob::of(&original));
    assert_eq!(first.made_on, Some(Blob::of(&[2; 20])));
    assert_eq!(ws.store().get(first.made_on()).unwrap(), [2; 20]);
    assert!(ws.stale(&game).unwrap().is_empty());

    // A resource of the same stream the mod left alone is made on the
    // original, which is not written down.
    let strings = key(&game, LEVEL, STRINGS);
    let mut table =
        crate::resource::lh2::StringTable::parse(&game_data(&game, LEVEL, STRINGS)).unwrap();
    table.strings[0][0] = "Edited".into();
    set(&mut ws, &game, LEVEL, STRINGS, table.to_bytes().unwrap());
    assert_eq!(ws.change(&strings).unwrap().made_on, None);

    // A later edit of the resource keeps what the first one was made on.
    set(&mut ws, &game, LEVEL, MESH, vec![4; 40]);
    let second = ws.change(&mesh).unwrap().clone();
    assert_eq!(second.made_on, first.made_on);
    assert_eq!(second.payload, Blob::of(&[4; 40]));

    // Undo and redo carry it; so does the workspace on disk.
    ws.undo().unwrap();
    assert_eq!(ws.change(&mesh), Some(&first));
    ws.undo().unwrap();
    ws.undo().unwrap();
    assert_eq!(ws.change(&mesh), None);
    for _ in 0..3 {
        ws.redo().unwrap();
    }
    assert_eq!(ws.change(&mesh), Some(&second));
    assert_eq!(ws.collect_garbage().unwrap(), 0);
    drop(ws);
    let mut ws = Workspace::open(&dir).unwrap();
    assert_eq!(ws.change(&mesh), Some(&second));
    assert_eq!(ws.store().get(second.made_on()).unwrap(), [2; 20]);
    let manifest = fs::read_to_string(dir.join("manifest.json")).unwrap();
    assert_eq!(manifest.matches("made_on").count(), 1, "{manifest}");

    // The data the game gives without the change is no change: here the
    // mod's. The original is one — it puts the mod's change back.
    set(&mut ws, &game, LEVEL, MESH, vec![2; 20]);
    assert_eq!(ws.change(&mesh), None);
    ws.undo().unwrap();
    set(&mut ws, &game, LEVEL, MESH, original.clone());
    let back = ws.change(&mesh).unwrap();
    assert_eq!(back.payload, Blob::of(&original));
    assert_eq!(back.made_on, first.made_on);

    // Removed and made again, the change is made on what the game gives then.
    ws.revert(std::slice::from_ref(&mesh), "revert").unwrap();
    game_gets(&game, LEVEL, MESH, &[5; 50]);
    set(&mut ws, &game, LEVEL, MESH, vec![6; 60]);
    assert_eq!(ws.change(&mesh).unwrap().made_on, Some(Blob::of(&[5; 50])));
}

#[test]
fn a_workspace_made_before_this_stage_loads_and_counts_as_made_on_the_original() {
    let (_dir, game) = town();
    let temp = tempfile::tempdir().unwrap();
    let mesh = key(&game, LEVEL, MESH);
    let original = game_data(&game, LEVEL, MESH);
    game_gets(&game, LEVEL, MESH, &[2; 20]);
    let dir = temp.path().join("old");
    let mut ws = Workspace::create(&dir, &game).unwrap();
    set(&mut ws, &game, LEVEL, MESH, vec![3; 30]);
    let recorded = ws.change(&mesh).unwrap().clone();
    drop(ws);
    forget_made_on(&dir.join("manifest.json"));
    forget_made_on(&dir.join("log.json"));

    let mut ws = Workspace::open(&dir).unwrap();
    let change = ws.change(&mesh).unwrap();
    assert_eq!(change.made_on, None);
    assert_eq!(change.payload, recorded.payload);
    // What it was made on is not known, so it is taken as the original:
    // the game gives other data, and the author says which stands.
    assert_eq!(
        ws.stale(&game).unwrap(),
        [Stale {
            key: mesh.clone(),
            made_on: Blob::of(&original),
            now: Blob::of(&[2; 20]),
            kind: None,
        }]
    );
    let report = ws.keep_mine(&game, std::slice::from_ref(&mesh)).unwrap();
    assert_eq!(report.changes[0].outcome, RebaseOutcome::Kept);
    assert_eq!(ws.change(&mesh), Some(&recorded));
    assert!(ws.stale(&game).unwrap().is_empty());
}

#[test]
fn a_resource_without_elements_is_kept_or_given_up() {
    let (_dir, game) = town();
    let temp = tempfile::tempdir().unwrap();
    let level = stream(&game, LEVEL);
    let mesh = key(&game, LEVEL, MESH);
    let original = game_data(&game, LEVEL, MESH);
    game_gets(&game, LEVEL, MESH, &[2; 20]);
    let mut ws = Workspace::create(temp.path().join("ws"), &game).unwrap();
    set(&mut ws, &game, LEVEL, MESH, vec![3; 30]);
    let before = ws.change(&mesh).unwrap().clone();

    // The mod is updated.
    game_gets(&game, LEVEL, MESH, &[5; 50]);
    let stale = ws.stale(&game).unwrap();
    assert_eq!(
        stale,
        [Stale {
            key: mesh.clone(),
            made_on: Blob::of(&[2; 20]),
            now: Blob::of(&[5; 50]),
            kind: None,
        }]
    );
    // What the game changed under the change, for the comparison views.
    let detail = ws
        .base_detail(&game, &Given::new(), &mesh)
        .unwrap()
        .unwrap();
    assert_eq!((detail.bytes.old_len, detail.bytes.new_len), (20, 50));

    // A mesh is not merged: nothing is recorded, and the report says so.
    let head = ws.head();
    let report = ws.rebase(&game, None).unwrap();
    assert_eq!(report.seq, None);
    let [
        Rebased {
            key: rebased,
            outcome,
        },
    ] = &report.changes[..]
    else {
        panic!("{report:?}");
    };
    assert_eq!(*rebased, mesh);
    assert!(
        matches!(outcome, RebaseOutcome::Undecided { reason } if reason.contains("mesh")),
        "{outcome:?}"
    );
    assert_eq!(report.undecided().count(), 1);
    assert!(report.describe()[0].contains("needs your decision"));
    assert_eq!((ws.head(), ws.change(&mesh)), (head, Some(&before)));

    // Keep mine: the change stays, made on what the game gives now.
    let report = ws.keep_mine(&game, std::slice::from_ref(&mesh)).unwrap();
    assert!(report.seq.is_some());
    assert_eq!(report.changes[0].outcome, RebaseOutcome::Kept);
    let kept = ws.change(&mesh).unwrap().clone();
    assert_eq!(kept.payload, before.payload);
    assert_eq!(kept.made_on, Some(Blob::of(&[5; 50])));
    assert_eq!(kept.label, before.label);
    assert!(ws.stale(&game).unwrap().is_empty());
    assert_eq!(ws.head(), head + 1);
    // Asked again, there is nothing to do and nothing is recorded.
    let again = ws.keep_mine(&game, std::slice::from_ref(&mesh)).unwrap();
    assert_eq!(
        (again.seq, &again.changes[0].outcome),
        (None, &RebaseOutcome::NotStale)
    );
    assert_eq!(ws.rebase(&game, None).unwrap(), RebaseReport::default());
    assert_eq!(ws.head(), head + 1);
    // One undo takes it back exactly.
    ws.undo().unwrap();
    assert_eq!(ws.change(&mesh), Some(&before));
    assert_eq!(ws.stale(&game).unwrap(), stale);

    // Take the base's: the change is removed, and comes back with undo.
    let report = ws.take_base(&game, std::slice::from_ref(&mesh)).unwrap();
    assert_eq!(report.changes[0].outcome, RebaseOutcome::Taken);
    assert_eq!(ws.change(&mesh), None);
    assert_eq!(ws.current_data(&game, level, MESH).unwrap(), [5; 50]);
    ws.undo().unwrap();
    assert_eq!(ws.change(&mesh), Some(&before));

    // The mod is gone: kept, the change is made on the original again,
    // which is not written down.
    game_gets(&game, LEVEL, MESH, &original);
    assert_eq!(ws.stale(&game).unwrap()[0].now, Blob::of(&original));
    ws.keep_mine(&game, std::slice::from_ref(&mesh)).unwrap();
    assert_eq!(ws.change(&mesh).unwrap().made_on, None);
    assert!(ws.stale(&game).unwrap().is_empty());

    // What the workspace does not change cannot be rebased, and a change
    // is not kept and given up at once.
    let other = key(&game, LEVEL, MATERIAL);
    assert!(ws.keep_mine(&game, std::slice::from_ref(&other)).is_err());
    let both = RebaseRequest {
        keys: None,
        keep: std::slice::from_ref(&mesh),
        take_base: std::slice::from_ref(&mesh),
    };
    assert!(ws.rebase_given(&game, &Given::new(), &both).is_err());
}

#[test]
fn a_change_that_adds_what_the_game_has_now_needs_a_decision() {
    let (_dir, game) = town();
    let temp = tempfile::tempdir().unwrap();
    let yard_key = key(&game, YARD, YARD_SIMGROUP);
    let pristine = game_data(&game, YARD, YARD_SIMGROUP);
    let with_21 = edit_group(&pristine, |g| add_crate(g, 21));
    game_gets(&game, YARD, YARD_SIMGROUP, &with_21);
    let mut ws = Workspace::create(temp.path().join("ws"), &game).unwrap();
    let mine = edit_group(&with_21, |g| add_crate(g, 22));
    set(&mut ws, &game, YARD, YARD_SIMGROUP, mine);
    let before = ws.change(&yard_key).unwrap().clone();

    // The mod now adds an object of the same GUID: both cannot be right.
    let now = edit_group(&with_21, |g| {
        add_crate(g, 22);
        g.set_position(object(g, 22), [7.0, 7.0, 7.0]).unwrap();
    });
    game_gets(&game, YARD, YARD_SIMGROUP, &now);
    assert_eq!(ws.stale(&game).unwrap()[0].kind, Some(Kind::SimGroup));
    let report = ws.rebase(&game, None).unwrap();
    assert_eq!(report.seq, None);
    let RebaseOutcome::Undecided { reason } = &report.changes[0].outcome else {
        panic!("{report:?}");
    };
    assert!(
        reason.contains(&guid(22).to_string()) && reason.contains("already in the group"),
        "{reason}"
    );
    assert_eq!(ws.change(&yard_key), Some(&before));
    // The author takes the mod's object.
    ws.take_base(&game, std::slice::from_ref(&yard_key))
        .unwrap();
    assert_eq!(ws.change(&yard_key), None);
}

#[test]
fn a_change_of_which_nothing_is_left_is_removed() {
    let (_dir, game) = town();
    let temp = tempfile::tempdir().unwrap();
    let yard_key = key(&game, YARD, YARD_SIMGROUP);
    let pristine = game_data(&game, YARD, YARD_SIMGROUP);
    let with_21 = edit_group(&pristine, |g| add_crate(g, 21));
    game_gets(&game, YARD, YARD_SIMGROUP, &with_21);
    let mut ws = Workspace::create(temp.path().join("ws"), &game).unwrap();
    // The one edit: the mod's crate is moved.
    let mine = edit_group(&with_21, |g| {
        g.set_position(object(g, 21), [1.0, 2.0, 3.0]).unwrap();
    });
    set(&mut ws, &game, YARD, YARD_SIMGROUP, mine);
    let before = ws.change(&yard_key).unwrap().clone();

    // The mod no longer has its crate.
    game_gets(&game, YARD, YARD_SIMGROUP, &pristine);
    let report = ws.rebase(&game, None).unwrap();
    let RebaseOutcome::Merged {
        applied,
        skipped,
        conflicts,
        removed,
    } = &report.changes[0].outcome
    else {
        panic!("{report:?}");
    };
    assert_eq!((*applied, conflicts.len(), *removed), (0, 0, true));
    assert_eq!(skipped.len(), 1);
    assert!(
        skipped[0].target.contains(&guid(21).to_string()),
        "{skipped:?}"
    );
    assert_eq!(skipped[0].reason, "the object is not in the group");
    assert!(report.describe()[0].contains("it is removed"));
    assert_eq!((report.skipped(), report.conflicts()), (1, 0));
    assert_eq!(ws.change(&yard_key), None);
    ws.undo().unwrap();
    assert_eq!(ws.change(&yard_key), Some(&before));
}

#[test]
fn merging_keeping_and_giving_up_are_one_operation() {
    let (_dir, game) = town();
    let temp = tempfile::tempdir().unwrap();
    let yard_key = key(&game, YARD, YARD_SIMGROUP);
    let mesh = key(&game, LEVEL, MESH);
    let material = key(&game, LEVEL, MATERIAL);
    let pristine = game_data(&game, YARD, YARD_SIMGROUP);
    let with_21 = edit_group(&pristine, |g| add_crate(g, 21));
    game_gets(&game, YARD, YARD_SIMGROUP, &with_21);
    game_gets(&game, LEVEL, MESH, &[2; 20]);
    game_gets(&game, LEVEL, MATERIAL, &[2; 21]);
    let mut ws = Workspace::create(temp.path().join("ws"), &game).unwrap();
    let mine = edit_group(&with_21, |g| {
        add_crate(g, 22);
        g.set_position(object(g, 4), [1.0, 2.0, 3.0]).unwrap();
    });
    set(&mut ws, &game, YARD, YARD_SIMGROUP, mine);
    set(&mut ws, &game, LEVEL, MESH, vec![3; 30]);
    set(&mut ws, &game, LEVEL, MATERIAL, vec![3; 31]);
    let before: Vec<Change> = ws.changes().cloned().collect();

    // Everything under the three changes is other data now: the mod moves
    // the crate the workspace moved as well and adds one more.
    let now = edit_group(&with_21, |g| {
        add_crate(g, 23);
        g.set_position(object(g, 4), [9.0, 9.0, 9.0]).unwrap();
    });
    game_gets(&game, YARD, YARD_SIMGROUP, &now);
    game_gets(&game, LEVEL, MESH, &[5; 50]);
    game_gets(&game, LEVEL, MATERIAL, &[5; 51]);
    assert_eq!(ws.stale(&game).unwrap().len(), 3);

    let head = ws.head();
    let request = RebaseRequest {
        keys: None,
        keep: std::slice::from_ref(&mesh),
        take_base: std::slice::from_ref(&material),
    };
    let report = ws.rebase_given(&game, &Given::new(), &request).unwrap();
    assert!(report.seq.is_some());
    assert_eq!(ws.head(), head + 1, "one operation");
    assert_eq!(ws.undo_label(), Some("Rebase 3 changes"));
    let outcome = |key: &ResourceKey| {
        &report
            .changes
            .iter()
            .find(|c| c.key == *key)
            .unwrap()
            .outcome
    };
    assert_eq!(*outcome(&mesh), RebaseOutcome::Kept);
    assert_eq!(*outcome(&material), RebaseOutcome::Taken);
    // Both moved the crate: the author's place stands and the report names
    // the object.
    let RebaseOutcome::Merged {
        applied,
        skipped,
        conflicts,
        removed,
    } = outcome(&yard_key)
    else {
        panic!("{report:?}");
    };
    assert_eq!((*applied, skipped.len(), *removed), (2, 0, false));
    assert_eq!(conflicts.len(), 1);
    assert!(
        conflicts[0].target.contains(&guid(4).to_string())
            && conflicts[0].target.contains("CMD_LoadMatrix"),
        "{conflicts:?}"
    );
    assert_eq!(report.merged().count(), 1);
    let group = SimGroup::parse(&ws.data(&yard_key).unwrap().unwrap()).unwrap();
    assert!(has(&group, 21) && has(&group, 22) && has(&group, 23));
    let moved = &group.objects[object(&group, 4)];
    assert_eq!(group.transform(moved).unwrap()[3][..3], [1.0, 2.0, 3.0]);
    assert_eq!(ws.change(&yard_key).unwrap().made_on, Some(Blob::of(&now)));
    assert_eq!(ws.change(&material), None);
    assert!(ws.stale(&game).unwrap().is_empty());

    // One undo: every change as it was before.
    ws.undo().unwrap();
    assert_eq!(ws.changes().cloned().collect::<Vec<_>>(), before);
    ws.redo().unwrap();
    assert!(ws.stale(&game).unwrap().is_empty());

    // Only the named changes are merged when keys are given.
    ws.undo().unwrap();
    let report = ws
        .rebase(&game, Some(std::slice::from_ref(&yard_key)))
        .unwrap();
    assert_eq!(report.changes.len(), 1);
    assert_eq!(ws.stale(&game).unwrap().len(), 2);
}

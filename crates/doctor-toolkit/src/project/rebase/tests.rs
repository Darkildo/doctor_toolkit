//! Rebasing with real mods on the miniature game: a project made on a base
//! mod that is updated, from the stale edit to the mod of the project in
//! the game above the new base; the same for strings in a workspace
//! without a project.

use std::path::{Path, PathBuf};

use crate::game::GameInstance;
use crate::mods::fold::tests::{
    add_crate, edit_group, edit_table, guid, has, info, object, set_text, town, workspace, yard_mod,
};
use crate::mods::manager::{ModApplyOptions, Payloads};
use crate::mods::package::ModArchive;
use crate::mods::{ModInfo, ModManager, add_suggested_dependencies, build_from_workspace};
use crate::project::{BaseDifference, NewDocument, Project, ProjectConfig, create_document};
use crate::resource::delta::{Delta, Kind};
use crate::resource::lh2::StringTable;
use crate::resource::simgroup::SimGroup;
use crate::testing::{LEVEL, STRINGS, YARD, YARD_SIMGROUP};
use crate::workspace::tests::{edit, game_data, key, stream};
use crate::workspace::{Blob, Given, RebaseOutcome, RebaseRequest, SemanticDiff, Stale, Workspace};

/// What each delta of the mod file `path` addresses, operation by operation.
fn delta_targets(path: &Path) -> Vec<Vec<String>> {
    let mut archive = ModArchive::open(path).unwrap();
    let deltas: Vec<_> = archive.manifest().deltas().cloned().collect();
    deltas
        .iter()
        .map(|resource| {
            let data = archive.read_payload(&resource.payload).unwrap();
            Delta::parse(&data).unwrap().targets()
        })
        .collect()
}

fn write_mods(game: &GameInstance) {
    ModManager::open(game)
        .unwrap()
        .apply(game, ModApplyOptions::default())
        .unwrap();
}

fn install(game: &GameInstance, file: &Path) {
    let mut mods = ModManager::open(game).unwrap();
    let report = mods.install_file(game, file, true).unwrap();
    assert!(report.issues.is_empty(), "{:?}", report.issues);
    write_mods(game);
}

#[test]
fn a_project_is_rebased_onto_the_update_of_its_base() {
    let (dir, game) = town();
    let temp = tempfile::tempdir().unwrap();
    let yard = stream(&game, YARD);
    let yard_key = key(&game, YARD, YARD_SIMGROUP);
    let yard_of =
        |game: &GameInstance| SimGroup::parse(&game_data(game, YARD, YARD_SIMGROUP)).unwrap();
    // A is the crate 21 of the base, B the crate 22 of the project, C the
    // crate 3 of the game and D the crate 23 of the base's update.
    let city_1 = yard_mod(&game, temp.path(), &info("city", "1.0.0"), |g| {
        add_crate(g, 21)
    });
    let city_2 = yard_mod(&game, temp.path(), &info("city", "1.1.0"), |g| {
        add_crate(g, 21);
        add_crate(g, 23);
        g.remove_object(object(g, 3)).unwrap();
    });
    install(&game, &city_1);
    let game = GameInstance::open(dir.path()).unwrap();
    let with_city_1 = game_data(&game, YARD, YARD_SIMGROUP);

    // The project, made on the game with the base: it adds B and moves C.
    let root = temp.path().join("Extension");
    let new = NewDocument {
        name: "Extension".into(),
        ..Default::default()
    };
    create_document(&game, &root, &new).unwrap();
    ProjectConfig::record_base(&root, &game).unwrap();
    let project = Project::open(&root).unwrap();
    let mut ws = project.workspace(&game).unwrap();
    let data = ws.current_data(&game, yard, YARD_SIMGROUP).unwrap();
    let mine = edit_group(&data, |g| {
        add_crate(g, 22);
        g.set_position(object(g, 3), [1.0, 2.0, 3.0]).unwrap();
    });
    ws.set_resources(
        &game,
        "a crate and a move",
        yard,
        &[edit(YARD_SIMGROUP, mine)],
    )
    .unwrap();
    let made = ws.change(&yard_key).unwrap().clone();
    assert_eq!(made.made_on, Some(Blob::of(&with_city_1)));
    assert!(project.stale(&game, &ws).unwrap().is_empty());
    let out = temp.path().join("extension-1.zip");
    let report = project.build_mod_on(&game, &ws, &out, None).unwrap();
    assert!(
        report.stale.is_empty() && report.notes.is_empty(),
        "{report:?}"
    );
    assert_eq!(report.deltas, 1);

    // The base is updated: it also adds D and no longer has C.
    install(&game, &city_2);
    let game = GameInstance::open(dir.path()).unwrap();
    let with_city_2 = game_data(&game, YARD, YARD_SIMGROUP);
    let now = yard_of(&game);
    assert!(has(&now, 21) && has(&now, 23) && !now.objects.iter().any(|o| o.guid == guid(3)));
    assert!(matches!(
        project.config.check_base(&game).unwrap()[..],
        [BaseDifference::Changed { .. }]
    ));

    // The edit stands on what the base gave before.
    let stale = project.stale(&game, &ws).unwrap();
    assert_eq!(
        stale,
        [Stale {
            key: yard_key.clone(),
            made_on: Blob::of(&with_city_1),
            now: Blob::of(&with_city_2),
            kind: Some(Kind::SimGroup),
        }]
    );
    assert!(crate::project::stale_warning(&stale).is_some());
    // Built as it is, the mod would take the update back — and says so.
    let out = temp.path().join("extension-stale.zip");
    let report = project.build_mod_on(&game, &ws, &out, None).unwrap();
    assert_eq!(report.stale, std::slice::from_ref(&yard_key));
    assert!(
        report.notes[0].contains("other data than the mod is built on")
            && report.notes[0].contains("project rebase")
            && report.notes[0].contains(&yard_key.to_string()),
        "{:?}",
        report.notes
    );
    // Whole or as a delta, it holds the yard without D and with C.
    let stale_mod = SimGroup::parse(&ws.data(&yard_key).unwrap().unwrap()).unwrap();
    assert!(!has(&stale_mod, 23) && has(&stale_mod, 3));

    // Rebased: the operations of the edit on what the base gives now.
    let head = ws.head();
    let report = project
        .rebase(&game, &mut ws, &RebaseRequest::default())
        .unwrap();
    assert!(report.seq.is_some());
    assert_eq!(ws.head(), head + 1);
    let [rebased] = &report.changes[..] else {
        panic!("{report:?}");
    };
    assert_eq!(rebased.key, yard_key);
    let RebaseOutcome::Merged {
        applied,
        skipped,
        conflicts,
        removed,
    } = &rebased.outcome
    else {
        panic!("{report:?}");
    };
    assert_eq!((*applied, conflicts.len(), *removed), (1, 0, false));
    // The move of C is left out, by object and reason.
    assert_eq!(skipped.len(), 1);
    assert_eq!(
        skipped[0].target,
        format!("object {}, CSystemCommands.CMD_LoadMatrix", guid(3))
    );
    assert_eq!(skipped[0].reason, "the object is not in the group");
    let lines = report.describe();
    assert!(
        lines[0].contains("1 operation(s) applied, 1 left out") && lines[1].contains("left out"),
        "{lines:?}"
    );
    let group = SimGroup::parse(&ws.data(&yard_key).unwrap().unwrap()).unwrap();
    assert!(has(&group, 21) && has(&group, 22) && has(&group, 23));
    assert!(!has(&group, 3), "C is gone with the base");
    let rebased = ws.change(&yard_key).unwrap().clone();
    assert_eq!(rebased.made_on, Some(Blob::of(&with_city_2)));
    assert_eq!(rebased.original, made.original);
    assert_eq!(rebased.label, made.label);
    assert!(project.stale(&game, &ws).unwrap().is_empty());
    // Nothing more to do: nothing more is recorded.
    let again = project
        .rebase(&game, &mut ws, &RebaseRequest::default())
        .unwrap();
    assert_eq!((again.seq, again.changes.len()), (None, 0));
    assert_eq!(ws.head(), head + 1);

    // One undo gives the state before exactly; redo, the rebased one.
    ws.undo().unwrap();
    assert_eq!(ws.change(&yard_key), Some(&made));
    assert_eq!(project.stale(&game, &ws).unwrap().len(), 1);
    ws.redo().unwrap();
    assert_eq!(ws.change(&yard_key), Some(&rebased));

    // The mod of the rebased project: a delta of the one object it adds,
    // which neither removes D nor puts C back.
    let out = temp.path().join("extension-2.zip");
    let report = project.build_mod_on(&game, &ws, &out, None).unwrap();
    assert!(
        report.stale.is_empty() && report.notes.is_empty(),
        "{report:?}"
    );
    assert_eq!(report.deltas, 1);
    assert_eq!(delta_targets(&out), [[format!("object {}", guid(22))]]);

    // Installed above the new base, the game has A, B and D, without C.
    let mut mods = ModManager::open(&game).unwrap();
    let installed = project
        .install_into(&game, &mut mods, &ws, Payloads::Copied)
        .unwrap();
    assert!(installed.warnings.is_empty(), "{:?}", installed.warnings);
    assert!(installed.issues.is_empty(), "{:?}", installed.issues);
    assert!(mods.check(&game).unwrap().is_empty());
    drop(mods);
    write_mods(&game);
    let game = GameInstance::open(dir.path()).unwrap();
    let group = yard_of(&game);
    assert!(has(&group, 21) && has(&group, 22) && has(&group, 23));
    assert!(!has(&group, 3));

    // The files of the game now hold the project's own mod. That is not
    // what its edit stands on: the edit is compared with the game without
    // it, where the file alone would call the edit stale.
    assert_eq!(ws.stale_given(&game, &Given::new()).unwrap().len(), 1);
    assert!(ws.stale(&game).unwrap().is_empty());
    assert_eq!(
        project.given(&game, &ws).unwrap(),
        Given::from([(yard_key.clone(), with_city_2.clone())])
    );
    assert!(project.stale(&game, &ws).unwrap().is_empty());
    let report = project
        .rebase(&game, &mut ws, &RebaseRequest::default())
        .unwrap();
    assert_eq!(report.seq, None);

    // An installation of stale changes is not silent either.
    ws.undo().unwrap();
    let mut mods = ModManager::open(&game).unwrap();
    let installed = project
        .install_into(&game, &mut mods, &ws, Payloads::Copied)
        .unwrap();
    assert_eq!(installed.warnings.len(), 1, "{:?}", installed.warnings);
    assert!(installed.warnings[0].contains("Rebase"));
}

/// Builds the mod `info` that edits the string table of the level of the
/// game as it is with `change`.
fn strings_mod(
    game: &GameInstance,
    temp: &Path,
    info: &ModInfo,
    change: impl FnOnce(&mut StringTable),
) -> PathBuf {
    let dir = temp.join(format!("ws-{}-{}", info.id, info.version));
    let ws = workspace(game, &dir, LEVEL, STRINGS, |data| edit_table(data, change));
    let out = temp.join(format!("{}-{}.zip", info.id, info.version));
    build_from_workspace(&ws, info, None, None, &out).unwrap();
    out
}

#[test]
fn strings_are_rebased_onto_the_update_of_the_mod_below() {
    let (dir, game) = town();
    let temp = tempfile::tempdir().unwrap();
    let level = stream(&game, LEVEL);
    let strings = key(&game, LEVEL, STRINGS);
    let text =
        |table: &StringTable, hash: u32| table.find(hash).map(|row| table.strings[0][row].clone());
    let words_1 = strings_mod(&game, temp.path(), &info("words", "1.0.0"), |t| {
        set_text(t, 0x100, "Base one");
        t.insert(0x200, &["Base adds".to_owned()]).unwrap();
    });
    // The update words another line, adds another string and no longer
    // adds the first one.
    let words_2 = strings_mod(&game, temp.path(), &info("words", "1.1.0"), |t| {
        set_text(t, 0x100, "Base two");
        t.insert(0x201, &["Base adds another".to_owned()]).unwrap();
    });
    install(&game, &words_1);
    let game = GameInstance::open(dir.path()).unwrap();
    let with_words_1 = game_data(&game, LEVEL, STRINGS);

    // A workspace of its own folder, no project: it changes a line of the
    // game, the two strings of the mod, and adds one.
    let mut ws = Workspace::create(temp.path().join("ws"), &game).unwrap();
    let data = ws.current_data(&game, level, STRINGS).unwrap();
    let mine = edit_table(&data, |t| {
        set_text(t, 0x100, "Mine");
        set_text(t, 0x101, "Mine too");
        set_text(t, 0x200, "Mine on the mod's");
        t.insert(0x300, &["Mine adds".to_owned()]).unwrap();
    });
    ws.set_resources(&game, "words", level, &[edit(STRINGS, mine)])
        .unwrap();
    assert_eq!(
        ws.change(&strings).unwrap().made_on,
        Some(Blob::of(&with_words_1))
    );

    install(&game, &words_2);
    let game = GameInstance::open(dir.path()).unwrap();
    let with_words_2 = game_data(&game, LEVEL, STRINGS);
    let stale = ws.stale(&game).unwrap();
    assert_eq!(stale.len(), 1);
    assert_eq!(
        (stale[0].kind, &stale[0].now),
        (Some(Kind::Lh2), &Blob::of(&with_words_2))
    );
    // What the mod changed under the edit, as the Changes panel shows it.
    let detail = ws.base_detail(&game, &Given::new(), &strings).unwrap();
    let Some(SemanticDiff::Strings(diff)) = detail.unwrap().semantic else {
        panic!("a comparison of strings is expected");
    };
    assert_eq!(
        (&diff.added[..], &diff.removed[..]),
        (&[0x201][..], &[0x200][..])
    );
    assert_eq!(
        (
            diff.changed[0].hash,
            diff.changed[0].old.as_str(),
            diff.changed[0].new.as_str()
        ),
        (0x100, "Base one", "Base two")
    );
    // A mod built now warns.
    let mut ext = info("ext", "1.0");
    assert_eq!(
        add_suggested_dependencies(&mut ext, &game, &ws, None).unwrap(),
        ["words"]
    );
    let out = temp.path().join("ext-stale.zip");
    let report = build_from_workspace(&ws, &ext, None, None, &out).unwrap();
    assert_eq!(report.stale, std::slice::from_ref(&strings));
    assert!(
        report.notes[0].contains("workspace rebase"),
        "{:?}",
        report.notes
    );

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
    assert_eq!((*applied, *removed), (3, false));
    // The string the mod no longer adds, and the line both worded.
    assert_eq!(skipped.len(), 1);
    assert_eq!(skipped[0].target, "string 00000200, language 0");
    assert_eq!(skipped[0].reason, "the table has no such string");
    assert_eq!(conflicts.len(), 1);
    assert_eq!(conflicts[0].target, "string 00000100, language 0");
    assert!(conflicts[0].reason.contains("Base two"), "{conflicts:?}");
    let table = StringTable::parse(&ws.data(&strings).unwrap().unwrap()).unwrap();
    assert_eq!(text(&table, 0x100).as_deref(), Some("Mine"));
    assert_eq!(text(&table, 0x101).as_deref(), Some("Mine too"));
    assert_eq!(text(&table, 0x200), None);
    assert_eq!(text(&table, 0x201).as_deref(), Some("Base adds another"));
    assert_eq!(text(&table, 0x300).as_deref(), Some("Mine adds"));
    assert!(ws.stale(&game).unwrap().is_empty());

    // The mod of the rebased workspace: three operations on the new data
    // of the mod below, and the game has the strings of both.
    let mut ext = info("ext", "1.0");
    add_suggested_dependencies(&mut ext, &game, &ws, None).unwrap();
    let out = temp.path().join("ext.zip");
    let report = build_from_workspace(&ws, &ext, None, None, &out).unwrap();
    assert!(report.stale.is_empty(), "{:?}", report.notes);
    assert_eq!(report.deltas, 1);
    assert_eq!(delta_targets(&out)[0].len(), 3);
    // The author installs the workspace as that mod to see it in the game.
    let mut mods = ModManager::open(&game).unwrap();
    let installed = mods.install_workspace(&game, &ws, &ext, None).unwrap();
    assert!(installed.warnings.is_empty(), "{:?}", installed.warnings);
    assert!(installed.issues.is_empty(), "{:?}", installed.issues);
    drop(mods);
    write_mods(&game);
    let game = GameInstance::open(dir.path()).unwrap();
    let table = StringTable::parse(&game_data(&game, LEVEL, STRINGS)).unwrap();
    assert_eq!(text(&table, 0x100).as_deref(), Some("Mine"));
    assert_eq!(text(&table, 0x201).as_deref(), Some("Base adds another"));
    assert_eq!(text(&table, 0x300).as_deref(), Some("Mine adds"));
    assert_eq!(text(&table, 0x200), None);

    // The files of the game hold the workspace's own mod now. The mod
    // manager remembers which workspace it was installed from, so the edit
    // is compared with the game without it and is not stale — the file
    // alone would say it is, and a rebase onto it would give the edit up
    // for its own installed copy.
    let mods = ModManager::open(&game).unwrap();
    assert_eq!(
        mods.get("ext").unwrap().workspace.as_deref(),
        Some(std::fs::canonicalize(ws.dir()).unwrap().as_path())
    );
    assert_eq!(mods.get("words").unwrap().workspace, None);
    assert_eq!(ws.stale_given(&game, &Given::new()).unwrap().len(), 1);
    assert_eq!(
        ws.given(&game).unwrap(),
        Given::from([(strings.clone(), with_words_2.clone())])
    );
    assert!(ws.stale(&game).unwrap().is_empty());
    let head = ws.head();
    assert_eq!(ws.rebase(&game, None).unwrap().seq, None);
    assert_eq!(ws.head(), head);
}

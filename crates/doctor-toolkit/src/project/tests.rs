use std::sync::atomic::AtomicBool;

use super::*;
use crate::navigation::unpack;
use crate::resource::lh2::StringTable;
use crate::resource::simgroup::SimGroup;
use crate::testing;

struct Fixture {
    _dir: tempfile::TempDir,
    game: GameInstance,
    project: Project,
}

fn fixture(settings: config::BuildSettings) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let game_dir = dir.path().join("game");
    std::fs::create_dir_all(&game_dir).unwrap();
    testing::write_game(&game_dir).unwrap();
    let game = GameInstance::open(&game_dir).unwrap();
    let ws = Workspace::for_game(&game, None).unwrap();
    let selection = unpack::select(&game, &[], &[]).unwrap();
    let options = CreateOptions {
        name: "Test Mod".into(),
        id: None,
        author: "me".into(),
        selection,
        settings,
    };
    let (project, report) = create(
        &game,
        &ws,
        None,
        &dir.path().join("project"),
        &options,
        &|_, _| {},
        &AtomicBool::new(false),
    )
    .unwrap();
    assert!(!report.cancelled);
    assert_eq!(report.resources, 10, "{report:?}");
    Fixture {
        _dir: dir,
        game,
        project,
    }
}

fn entry_of<'a>(project: &'a Project, type_tag: &str, stream: &str) -> (String, &'a Entry) {
    project
        .entries()
        .find(|(_, e, d)| e.type_tag == type_tag && d.contains(stream))
        .map(|(id, e, _)| (id, e))
        .unwrap_or_else(|| panic!("no {type_tag} entry"))
}

fn states(status: &ProjectStatus) -> Vec<(String, &'static str)> {
    status
        .entries
        .iter()
        .filter(|e| e.state != EntryState::Unchanged)
        .map(|e| (e.id.clone(), e.state.label()))
        .collect()
}

#[test]
fn a_project_holds_every_resource_in_its_format() {
    let f = fixture(Default::default());
    let p = &f.project;
    assert_eq!(p.config.project.id, "test-mod");
    let kinds: Vec<(String, Kind)> = p.entries().map(|(id, e, _)| (id, e.kind)).collect();
    let has = |suffix: &str, kind: Kind| {
        kinds
            .iter()
            .any(|(id, k)| id.ends_with(suffix) && *k == kind)
    };
    assert!(has(".png", Kind::Texture), "{kinds:?}");
    assert!(has(".glb", Kind::Model), "{kinds:?}");
    assert!(has(".json", Kind::Strings), "{kinds:?}");
    assert!(has(".json", Kind::Logic), "{kinds:?}");
    assert!(has(".view.json", Kind::View), "{kinds:?}");
    assert!(has(".bin", Kind::Raw), "{kinds:?}");
    // Folders by stream; the files of a texture and a model are one entry.
    assert!(
        p.folders
            .iter()
            .any(|f| f.dir == "src/gf2/level.str"
                && f.manifest.stream.as_deref() == Some("gf2/level"))
    );
    let (_, texture) = entry_of(p, "tg4h", "level");
    assert_eq!(texture.keys.len(), 2);
    assert_eq!(texture.policy, "default");
    let (_, model) = entry_of(p, "mesh", "level");
    assert_eq!(model.keys.len(), 2, "mesh and meshvolatile");
    // Opening reads the same project back.
    let again = Project::open(p.root()).unwrap();
    assert_eq!(again.folders, p.folders);
}

#[test]
fn building_writes_only_changed_files_into_the_workspace() {
    let f = fixture(Default::default());
    let (p, game) = (&f.project, &f.game);
    let mut ws = p.workspace(game).unwrap();
    let status = p.status(&ws).unwrap();
    assert!(states(&status).is_empty(), "{:?}", states(&status));
    assert!(
        status.unknown_files.is_empty(),
        "{:?}",
        status.unknown_files
    );

    // Nothing changed: nothing to build, no workspace.
    let report = p.build(game, &mut ws, &BuildOptions::default()).unwrap();
    assert!(
        report.built.is_empty() && report.errors.is_empty(),
        "{report:?}"
    );
    assert_eq!(report.seq, None);
    assert!(!ws.is_saved());

    // Edit a string, a texture and add an object by patch.
    let (strings_id, strings) = entry_of(p, "lh2", "text");
    let path = p.root().join(&strings_id);
    let mut doc: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    doc["entries"][0]["text"] = "Oui".into();
    std::fs::write(&path, serde_json::to_vec_pretty(&doc).unwrap()).unwrap();
    let (texture_id, texture) = entry_of(p, "tg4h", "level");
    let png = std::fs::read(p.root().join(&texture_id)).unwrap();
    let (w, h, mut rgba) = crate::resource::pixels::decode_png(&png).unwrap();
    for px in rgba.chunks_mut(4) {
        px[0] = 255 - px[0];
    }
    std::fs::write(
        p.root().join(&texture_id),
        crate::resource::pixels::encode_png(w, h, &rgba).unwrap(),
    )
    .unwrap();
    let (group_id, group) = entry_of(p, "simgroup", "level");
    let (_, dir) = p.entry(&group_id).unwrap();
    let patch = Project::patch_path(dir, group).unwrap();
    std::fs::create_dir_all(p.root().join(&patch).parent().unwrap()).unwrap();
    std::fs::write(
        p.root().join(&patch),
        format!(
            "[[new]]\nname = \"doctor_box\"\ncopy = \"{}\"\n\n\
             [[object]]\nguid = \"ffffffffffffffffffffffffffffffff\"\nposition = [0, 0, 0]\n",
            testing::OBJECT_GUID
        ),
    )
    .unwrap();

    let status = p.status(&ws).unwrap();
    let mut modified = states(&status);
    modified.sort();
    let mut expected = vec![
        (strings_id.clone(), "modified"),
        (texture_id.clone(), "modified"),
        (group_id.clone(), "modified"),
    ];
    expected.sort();
    assert_eq!(modified, expected);

    let report = p.build(game, &mut ws, &BuildOptions::default()).unwrap();
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert_eq!(report.built.len(), 3);
    assert!(report.seq.is_some());
    assert_eq!(ws.head(), 1, "one undoable operation");
    // The mini game's texture header gets its mip levels: only that may differ.
    let mut changed: Vec<String> = ws
        .changes()
        .map(|c| c.key.to_string())
        .filter(|k| *k != texture.keys[0].to_string())
        .collect();
    changed.sort();
    let mut expected = vec![
        strings.keys[0].to_string(),
        texture.keys[1].to_string(),
        group.keys[0].to_string(),
    ];
    expected.sort();
    assert_eq!(changed, expected);
    let table = StringTable::parse(&ws.data(&strings.keys[0]).unwrap().unwrap()).unwrap();
    assert_eq!(table.strings[0][0], "Oui");
    // The new object has a GUID from its name; the stale edit is a warning.
    let built_group = SimGroup::parse(&ws.data(&group.keys[0]).unwrap().unwrap()).unwrap();
    let new = built_group.objects.last().unwrap();
    assert_eq!(built_group.objects.len(), 2);
    let expected_guid = guid::derived("test-mod:simgroup:00000004", "doctor_box", |g| {
        SimGroup::parse(&cache::get_base(p.root(), group.base.as_deref().unwrap()).unwrap())
            .unwrap()
            .objects
            .iter()
            .any(|o| o.guid == *g)
    });
    assert_eq!(new.guid, expected_guid);
    assert_eq!(
        crate::resource::neoguid::parts(&new.guid).unwrap().0,
        crate::resource::neoguid::MachineName::TOOLKIT
    );
    let notes: Vec<&String> = report.built.iter().flat_map(|b| &b.notes).collect();
    assert!(
        notes
            .iter()
            .any(|n| n.contains("ffffffff") && n.contains("skipped")),
        "{notes:?}"
    );

    // Built: a second build does nothing, the status says built.
    let status = p.status(&ws).unwrap();
    assert_eq!(status.count(&EntryState::Built), 3, "{:?}", states(&status));
    let again = p.build(game, &mut ws, &BuildOptions::default()).unwrap();
    assert!(again.built.is_empty(), "{again:?}");
    assert_eq!(ws.head(), 1);

    // Building again gives the same GUID (from the name).
    let mut forced = BuildOptions {
        force: true,
        only: vec![group_id.clone()],
        ..Default::default()
    };
    let rebuilt = p.build(game, &mut ws, &forced).unwrap();
    assert_eq!(rebuilt.built.len(), 1);
    assert!(rebuilt.built[0].changed.is_empty(), "same data again");
    forced.force = false;

    // Undo in the workspace: out of sync; building writes the files again.
    ws.undo().unwrap();
    let status = p.status(&ws).unwrap();
    assert_eq!(
        status.count(&EntryState::OutOfSync),
        3,
        "{:?}",
        states(&status)
    );
    let report = p.build(game, &mut ws, &BuildOptions::default()).unwrap();
    assert_eq!(report.built.len(), 3);
    assert_eq!(
        ws.changes().count(),
        4,
        "with the header of the mini texture"
    );

    // A mod of the project holds what it changed.
    let out = p.root().parent().unwrap().join("test.zip");
    let mod_report = p.build_mod(&ws, &out, None).unwrap();
    assert_eq!(mod_report.resources, 4);
    let package = crate::mods::package::ModArchive::open(&out).unwrap();
    assert_eq!(package.manifest().id, "test-mod");
}

#[test]
fn a_broken_file_is_reported_with_its_path_and_the_rest_is_built() {
    let f = fixture(Default::default());
    let (p, game) = (&f.project, &f.game);
    let mut ws = p.workspace(game).unwrap();
    let (strings_id, _) = entry_of(p, "lh2", "text");
    std::fs::write(p.root().join(&strings_id), b"{ not json").unwrap();
    let (raw_id, raw) = entry_of(p, "material", "level");
    let mut bytes = std::fs::read(p.root().join(&raw_id)).unwrap();
    let last = bytes.len() - 1;
    bytes[last] ^= 1;
    std::fs::write(p.root().join(&raw_id), &bytes).unwrap();
    // A file the project does not know is listed, not built.
    std::fs::write(p.root().join("src/gf2/level.str/extra.png"), b"x").unwrap();
    let report = p.build(game, &mut ws, &BuildOptions::default()).unwrap();
    assert_eq!(report.errors.len(), 1, "{:?}", report.errors);
    assert!(
        report.errors[0].starts_with(&strings_id),
        "{}",
        report.errors[0]
    );
    assert!(report.errors[0].contains("JSON"), "{}", report.errors[0]);
    assert_eq!(report.built.len(), 1);
    assert_eq!(ws.data(&raw.keys[0]).unwrap().unwrap(), bytes);
    let status = p.status(&ws).unwrap();
    assert_eq!(status.unknown_files, ["src/gf2/level.str/extra.png"]);
    // A missing file stops only its entry.
    std::fs::remove_file(p.root().join(&raw_id)).unwrap();
    let report = p.build(game, &mut ws, &BuildOptions::default()).unwrap();
    assert!(
        report.errors.iter().any(|e| e.contains("missing")),
        "{report:?}"
    );
}

#[test]
fn edits_made_elsewhere_can_be_pulled_into_the_files() {
    let f = fixture(Default::default());
    let (p, game) = (&f.project, &f.game);
    let mut p = p.clone();
    let mut ws = p.workspace(game).unwrap();
    let (strings_id, strings) = entry_of(&p, "lh2", "text");
    let strings = strings.clone();
    let stream = game.find_stream(&strings.keys[0].stream).unwrap();
    let mut table = StringTable::parse(&ws.current_data(game, stream, 0).unwrap()).unwrap();
    table.strings[0][1] = "Non".into();
    ws.set_string_table(game, stream, 0, &table).unwrap();
    let status = p.status(&ws).unwrap();
    assert_eq!(states(&status), [(strings_id.clone(), "out of sync")]);
    let pulled = p.pull(game, &ws, &[]).unwrap();
    assert_eq!(pulled, std::slice::from_ref(&strings_id));
    let text = std::fs::read_to_string(p.root().join(&strings_id)).unwrap();
    assert!(text.contains("\"Non\""), "{text}");
    let status = Project::open(p.root()).unwrap().status(&ws).unwrap();
    assert!(states(&status).is_empty(), "{:?}", states(&status));
    // Building now changes nothing.
    let p = Project::open(p.root()).unwrap();
    let report = p.build(game, &mut ws, &BuildOptions::default()).unwrap();
    assert!(report.built.is_empty(), "{report:?}");
}

#[test]
fn a_project_from_a_workspace_holds_its_changes() {
    let dir = tempfile::tempdir().unwrap();
    let game_dir = dir.path().join("game");
    std::fs::create_dir_all(&game_dir).unwrap();
    testing::write_game(&game_dir).unwrap();
    let game = GameInstance::open(&game_dir).unwrap();
    let mut ws = Workspace::for_game(&game, None).unwrap();
    let stream = game.find_stream(testing::TEXT).unwrap();
    let mut table = StringTable::parse(&ws.current_data(&game, stream, 0).unwrap()).unwrap();
    table.strings[0][0] = "Si".into();
    ws.set_string_table(&game, stream, 0, &table).unwrap();
    let selection = workspace_selection(&game, &ws).unwrap();
    assert_eq!(selection, [(stream, vec![0])]);
    let (project, _) = create(
        &game,
        &ws,
        None,
        &dir.path().join("p"),
        &CreateOptions {
            name: "From WS".into(),
            id: None,
            author: String::new(),
            selection,
            settings: Default::default(),
        },
        &|_, _| {},
        &AtomicBool::new(false),
    )
    .unwrap();
    let ids: Vec<String> = project.entries().map(|(id, _, _)| id).collect();
    assert_eq!(ids.len(), 1, "{ids:?}");
    let text = std::fs::read_to_string(project.root().join(&ids[0])).unwrap();
    assert!(text.contains("\"Si\""), "{text}");
    let status = project.status(&ws).unwrap();
    assert!(states(&status).is_empty(), "{:?}", states(&status));
    // Undoing in the workspace: building puts the project's version back.
    ws.undo().unwrap();
    let report = project
        .build(&game, &mut ws, &BuildOptions::default())
        .unwrap();
    assert_eq!(report.built.len(), 1, "{report:?}");
    let key = &project.entries().next().unwrap().1.keys[0];
    let table = StringTable::parse(&ws.data(key).unwrap().unwrap()).unwrap();
    assert_eq!(table.strings[0][0], "Si");
    // A second project in the same folder is refused.
    assert!(
        create(
            &game,
            &ws,
            None,
            project.root(),
            &CreateOptions {
                name: "x".into(),
                id: None,
                author: String::new(),
                selection: Vec::new(),
                settings: Default::default(),
            },
            &|_, _| {},
            &AtomicBool::new(false),
        )
        .is_err()
    );
}

#[test]
fn shared_resources_are_written_once() {
    let dir = tempfile::tempdir().unwrap();
    let game_dir = dir.path().join("game");
    std::fs::create_dir_all(&game_dir).unwrap();
    testing::write_game(&game_dir).unwrap();
    // A second level stream with the same resources.
    std::fs::copy(
        game_dir.join("gf2/level.str"),
        game_dir.join("gf2/level2.str"),
    )
    .unwrap();
    let game = GameInstance::open(&game_dir).unwrap();
    let mut ws = Workspace::for_game(&game, None).unwrap();
    let level = game.find_stream(testing::LEVEL).unwrap();
    let level2 = game.find_stream("gf2/level2").unwrap();
    let all = |s: usize| (0..game.open_stream(s).unwrap().layout().resources.len()).collect();
    let (project, report) = create(
        &game,
        &ws,
        None,
        &dir.path().join("p"),
        &CreateOptions {
            name: "Shared".into(),
            id: None,
            author: String::new(),
            selection: vec![(level, all(level)), (level2, all(level2))],
            settings: Default::default(),
        },
        &|_, _| {},
        &AtomicBool::new(false),
    )
    .unwrap();
    assert!(report.shared >= 8, "{report:?}");
    let shared: Vec<(String, &Entry)> = project
        .entries()
        .filter(|(_, _, d)| *d == SHARED_DIR)
        .map(|(id, e, _)| (id, e))
        .collect();
    let (texture_id, texture) = shared
        .iter()
        .find(|(_, e)| e.kind == Kind::Texture)
        .unwrap();
    assert_eq!(texture.keys.len(), 4, "two streams × tg4h + tg4d");
    // A change of the shared file goes into both streams.
    let png = std::fs::read(project.root().join(texture_id)).unwrap();
    let (w, h, mut rgba) = crate::resource::pixels::decode_png(&png).unwrap();
    rgba[0] ^= 0xff;
    std::fs::write(
        project.root().join(texture_id),
        crate::resource::pixels::encode_png(w, h, &rgba).unwrap(),
    )
    .unwrap();
    let report = project
        .build(&game, &mut ws, &BuildOptions::default())
        .unwrap();
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    let streams: std::collections::BTreeSet<String> =
        ws.changes().map(|c| c.key.stream.clone()).collect();
    assert_eq!(
        streams.into_iter().collect::<Vec<_>>(),
        ["gf2/level", "gf2/level2"]
    );
}

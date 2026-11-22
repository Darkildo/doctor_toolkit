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

/// A mesh without meshvolatile (like the shadow meshes of the game): the
/// quad of the miniature game moved by `offset`, all its vertex data in the
/// mesh.
fn mesh_without_volatile(offset: f32) -> Vec<u8> {
    use crate::resource::mesh_build::{self, PartGeometry};
    use crate::resource::mesh_image::FLAG_VOLATILE;
    let part = PartGeometry {
        name: "shadow".into(),
        material_hash: crate::hash::crc32(testing::MATERIAL_NAME.as_bytes()),
        positions: vec![
            [offset, 0.0, 0.0],
            [offset + 1.0, 0.0, 0.0],
            [offset, 1.0, 0.0],
            [offset + 1.0, 1.0, 0.0],
        ],
        uv0: Some(vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0], [1.0, 1.0]]),
        triangles: vec![[0, 1, 2], [2, 1, 3]],
        ..Default::default()
    };
    let options = mesh_build::BuildOptions {
        name: "shadow".into(),
        ..Default::default()
    };
    let (mut image, _) = mesh_build::build(&[part], &options).unwrap();
    image.has_volatile = false;
    for buffer in &mut image.vertex_buffers {
        buffer.flags &= !FLAG_VOLATILE;
    }
    let (mesh, volatile) = image.encode().unwrap();
    assert!(volatile.is_empty());
    mesh
}

/// The shadow meshes of the game: the same mesh in several streams (an
/// entry of `shared/`) and, under the same ids, another one in a third
/// stream (an entry of its own), none with a meshvolatile; a fourth stream
/// with a copy is not in the project. A forced build writes each resource
/// from one entry: an entry does not replace the copies of its mesh that
/// other entries (or its own other keys) build, and the copy outside the
/// project gets the mesh of one entry.
#[test]
fn shared_meshes_without_volatile_are_built_once() {
    use crate::container::ustream::{
        FileHeader, ResourceHeader, Storage, StreamLayout, StreamWriter,
    };
    use crate::resource::mesh::{TYPE_MESH, TYPE_MESH_VOLATILE};
    let dir = tempfile::tempdir().unwrap();
    let game_dir = dir.path().join("game");
    std::fs::create_dir_all(&game_dir).unwrap();
    testing::write_game(&game_dir).unwrap();
    // The texture and the material of the level, and a mesh after them.
    let level = std::fs::read(game_dir.join("gf2/level.str")).unwrap();
    let layout = StreamLayout::parse(&level).unwrap();
    let write = |name: &str, mesh: &[u8]| {
        let mut out = StreamWriter::new(&FileHeader::GF2_PC, crate::io::Endian::Little);
        for r in [testing::TEX_HEADER, testing::TEX_DATA, testing::MATERIAL] {
            let header = layout.resources[r].header;
            let data = layout.resource_data(&level, r).unwrap();
            out.push_resource(&header, &data, Storage::Raw).unwrap();
        }
        let header = ResourceHeader {
            mem_policy: crate::hash::sdbm(b"geometry"),
            alignment: 0x80,
            type_id: TYPE_MESH,
            ids: [0x5151; 4],
            data_size: 0,
        };
        out.push_resource(&header, mesh, Storage::RefPack).unwrap();
        std::fs::write(game_dir.join(format!("gf2/{name}.str")), out.finish()).unwrap();
    };
    let shadow = mesh_without_volatile(0.0);
    write("car_a", &shadow);
    write("car_b", &shadow);
    write("car_c", &mesh_without_volatile(5.0));
    write("car_d", &shadow);
    let game = GameInstance::open(&game_dir).unwrap();
    let mut ws = Workspace::for_game(&game, None).unwrap();
    let selection = ["gf2/car_a", "gf2/car_b", "gf2/car_c"]
        .iter()
        .map(|s| {
            let s = game.find_stream(s).unwrap();
            let mesh = game.open_stream(s).unwrap().layout().resources.len() - 1;
            (s, vec![mesh])
        })
        .collect();
    let (project, _) = create(
        &game,
        &ws,
        None,
        &dir.path().join("p"),
        &CreateOptions {
            name: "Shadows".into(),
            id: None,
            author: String::new(),
            selection,
            settings: Default::default(),
        },
        &|_, _| {},
        &AtomicBool::new(false),
    )
    .unwrap();
    let entries: Vec<(String, Entry)> = project
        .entries()
        .map(|(id, e, _)| (id, e.clone()))
        .collect();
    assert_eq!(entries.len(), 2, "{entries:?}");
    let (shared_id, shared) = entries
        .iter()
        .find(|(id, _)| id.starts_with(SHARED_DIR))
        .unwrap();
    assert_eq!(shared.kind, Kind::Model);
    assert_eq!(shared.keys.len(), 2, "the mesh in car_a and car_b");

    let report = project
        .build(
            &game,
            &mut ws,
            &BuildOptions {
                force: true,
                ..BuildOptions::default()
            },
        )
        .unwrap();
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert_eq!(report.built.len(), 2);
    let changed = |id: &str| -> Vec<String> {
        let built = report.built.iter().find(|b| b.id == id).unwrap();
        built
            .changed
            .iter()
            .map(|k| format!("{}:{:x}", k.stream, k.type_id))
            .filter(|k| !k.starts_with("gf2/car_d:"))
            .collect()
    };
    // Each entry writes its meshes and the meshvolatile each gets (the
    // builder keeps the second vertex stream there), in its own streams.
    let (mesh, volatile) = (TYPE_MESH, TYPE_MESH_VOLATILE);
    assert_eq!(
        changed(shared_id),
        [
            format!("gf2/car_a:{mesh:x}"),
            format!("gf2/car_a:{volatile:x}"),
            format!("gf2/car_b:{mesh:x}"),
            format!("gf2/car_b:{volatile:x}"),
        ]
    );
    let own = entries.iter().find(|(id, _)| id != shared_id).unwrap();
    assert_eq!(
        changed(&own.0),
        [
            format!("gf2/car_c:{mesh:x}"),
            format!("gf2/car_c:{volatile:x}")
        ]
    );
    // The copy outside the project: from one entry, mesh and meshvolatile.
    let outside: Vec<(&str, u32)> = report
        .built
        .iter()
        .flat_map(|b| b.changed.iter().map(move |k| (b.id.as_str(), k)))
        .filter(|(_, k)| k.stream == "gf2/car_d")
        .map(|(id, k)| (id, k.type_id))
        .collect();
    assert_eq!(outside.len(), 2, "{outside:?}");
    assert_eq!(outside[0].0, outside[1].0, "{outside:?}");
    assert_eq!([outside[0].1, outside[1].1], [mesh, volatile]);
}

/// A game in a temporary folder and one edited string of its workspace in
/// `ws_dir` (the game's own when `None`).
fn game_with_an_edit(dir: &Path, ws_dir: Option<&Path>) -> (GameInstance, Workspace) {
    let game_dir = dir.join("game");
    std::fs::create_dir_all(&game_dir).unwrap();
    testing::write_game(&game_dir).unwrap();
    let game = GameInstance::open(&game_dir).unwrap();
    let mut ws = Workspace::for_game(&game, ws_dir).unwrap();
    let stream = game.find_stream(testing::TEXT).unwrap();
    let mut table = StringTable::parse(&ws.current_data(&game, stream, 0).unwrap()).unwrap();
    table.strings[0][0] = "Si".into();
    ws.set_string_table(&game, stream, 0, &table).unwrap();
    (game, ws)
}

/// Stage 113: the sets of zones are the project's (`[[zones]]`), written
/// without touching what the author wrote in `project.toml`; what the editor
/// loaded last is this computer's (`.doctor/editor.json`).
#[test]
fn zone_sets_and_the_editor_state_of_a_project() {
    use crate::level::ZoneSet;
    use crate::project::{EditorCamera, EditorState};

    let dir = tempfile::tempdir().unwrap();
    let game_dir = dir.path().join("game");
    std::fs::create_dir_all(&game_dir).unwrap();
    testing::write_game(&game_dir).unwrap();
    let game = GameInstance::open(&game_dir).unwrap();
    let root = dir.path().join("my mod");
    let new = NewDocument {
        name: "My Mod".into(),
        id: None,
        author: "me".into(),
    };
    let mut project = create_document(&game, &root, &new).unwrap();
    assert!(project.config.zones.is_empty());
    let file = root.join(PROJECT_FILE);
    let text = std::fs::read_to_string(&file).unwrap();
    assert!(!text.contains("zones"), "{text}");
    // The author's own lines stay where they are.
    let text = text.replace("[build]", "# where the edits live\n[build]");
    std::fs::write(&file, &text).unwrap();

    let yard = ZoneSet {
        name: "the yard".into(),
        streams: vec!["gf2/town/yard".into(), "gf2/town".into()],
    };
    let level = ZoneSet {
        name: "level one".into(),
        streams: vec!["gf2/level".into()],
    };
    project
        .set_zone_sets(vec![yard.clone(), level.clone()])
        .unwrap();
    let written = std::fs::read_to_string(&file).unwrap();
    assert!(written.starts_with(text.trim_end()), "{written}");
    assert!(written.contains("# where the edits live"));
    assert!(
        written.contains("[[zones]]\nname = \"the yard\""),
        "{written}"
    );
    assert_eq!(
        Project::open(&root).unwrap().config.zones,
        [yard.clone(), level]
    );

    // Replaced, not added to; a table written by hand between the sets and
    // after them stays.
    let by_hand = written.replace(
        "[[zones]]\nname = \"level",
        "[extra]\nkey = 1\n\n[[zones]]\nname = \"level",
    ) + "\n[more]\nkey = [\n  2,\n]\n";
    std::fs::write(&file, &by_hand).unwrap();
    project.set_zone_sets(vec![yard.clone()]).unwrap();
    let written = std::fs::read_to_string(&file).unwrap();
    assert_eq!(written.matches("[[zones]]").count(), 1, "{written}");
    assert!(written.contains("[extra]\nkey = 1") && written.contains("[more]\nkey = ["));
    assert_eq!(Project::open(&root).unwrap().config.zones, [yard]);
    project.set_zone_sets(Vec::new()).unwrap();
    let written = std::fs::read_to_string(&file).unwrap();
    assert!(!written.contains("zones") && written.contains("[more]"));
    assert!(Project::open(&root).unwrap().config.zones.is_empty());

    // The state of the editor: none at first, kept with the keys of a later
    // version, and out of version control also in a project whose
    // `.gitignore` is older than the file.
    assert_eq!(EditorState::read(&root), EditorState::default());
    let ignore = root.join(".gitignore");
    let old = std::fs::read_to_string(&ignore)
        .unwrap()
        .replace("/.doctor/editor.json\n", "");
    std::fs::write(&ignore, &old).unwrap();
    std::fs::write(
        root.join(".doctor/editor.json"),
        r#"{ "zones": ["preset:gf2/town"], "tabs": [1, 2] }"#,
    )
    .unwrap();
    let mut state = EditorState::read(&root);
    assert_eq!(state.zones, ["preset:gf2/town"]);
    state.zones = vec!["gf2/town/yard".into()];
    state.camera = Some(EditorCamera {
        eye: [1.0, 2.0, 3.0],
        yaw: 0.5,
        pitch: -0.25,
    });
    state.write(&root).unwrap();
    let again = EditorState::read(&root);
    assert_eq!(again, state);
    assert_eq!(again.other["tabs"], serde_json::json!([1, 2]));
    let ignored = std::fs::read_to_string(&ignore).unwrap();
    assert_eq!(
        ignored.replace("/.doctor/editor.json\n", ""),
        old,
        "{ignored}"
    );
    assert!(ignored.contains("/.doctor/local.toml\n/.doctor/editor.json\n"));
    state.write(&root).unwrap();
    assert_eq!(std::fs::read_to_string(&ignore).unwrap(), ignored);
}

#[test]
fn a_document_project_has_its_own_workspace_and_no_sources() {
    let dir = tempfile::tempdir().unwrap();
    let game_dir = dir.path().join("game");
    std::fs::create_dir_all(&game_dir).unwrap();
    testing::write_game(&game_dir).unwrap();
    let game = GameInstance::open(&game_dir).unwrap();
    let root = dir.path().join("my mod");
    let project = create_document(
        &game,
        &root,
        &NewDocument {
            name: "My Mod".into(),
            id: None,
            author: "me".into(),
        },
    )
    .unwrap();
    assert_eq!(project.config.project.id, "my-mod");
    assert!(project.has_own_workspace());
    assert_eq!(project.workspace_dir(), Some(root.join("workspace")));
    assert_eq!(project.entries().count(), 0);
    // The path of the game is this computer's: not in project.toml.
    let text = std::fs::read_to_string(root.join(PROJECT_FILE)).unwrap();
    assert!(text.contains("workspace = \"workspace\""), "{text}");
    assert!(!text.contains("path"), "{text}");
    assert_eq!(
        project.game_path(),
        Some(std::fs::canonicalize(&game_dir).unwrap())
    );
    let ignored = std::fs::read_to_string(root.join(".gitignore")).unwrap();
    assert!(ignored.contains("/.doctor/local.toml") && ignored.contains("/workspace/staged/"));
    assert!(ignored.contains("/.doctor/editor.json"), "{ignored}");

    // Opened again, it finds its game and an empty workspace that is not
    // on disk before the first edit.
    let project = Project::open(&root).unwrap();
    let game = project.game(None).unwrap();
    let mut ws = project.workspace(&game).unwrap();
    assert!(!ws.is_saved() && ws.is_empty());
    assert!(!root.join("workspace").exists());
    assert!(
        project
            .build_mod(&ws, &dir.path().join("empty.zip"), None)
            .is_err()
    );

    // An edit goes into the project's folder, not into the game's, and all
    // of the workspace is the project's mod.
    let stream = game.find_stream(testing::TEXT).unwrap();
    let mut table = StringTable::parse(&ws.current_data(&game, stream, 0).unwrap()).unwrap();
    table.strings[0][0] = "Si".into();
    ws.set_string_table(&game, stream, 0, &table).unwrap();
    assert!(Workspace::exists(&root.join("workspace")));
    assert!(!Workspace::default_dir(game.root()).exists());
    assert_eq!(project.changed_keys(&ws).len(), 1);
    let report = project
        .build_mod(&ws, &dir.path().join("mod.zip"), None)
        .unwrap();
    assert_eq!(report.resources, 1, "{report:?}");

    // A second project in the folder, a folder with files, an empty name.
    let again = NewDocument {
        name: "Other".into(),
        ..Default::default()
    };
    assert!(create_document(&game, &root, &again).is_err());
    assert!(create_document(&game, dir.path(), &again).is_err());
    assert!(create_document(&game, &dir.path().join("x"), &NewDocument::default()).is_err());

    // The game moved: the project is told where it is now.
    project.set_game_path(dir.path()).unwrap();
    assert_eq!(
        Project::open(&root).unwrap().game_path(),
        Some(std::fs::canonicalize(dir.path()).unwrap())
    );
}

#[test]
fn the_workspace_of_the_game_moves_into_a_project() {
    let dir = tempfile::tempdir().unwrap();
    let (game, mut ws) = game_with_an_edit(dir.path(), None);
    let from = ws.dir().to_owned();
    let (changes, operations) = (ws.changes().count(), ws.head());
    assert_eq!((changes, operations), (1, 1));
    let project = create_document(
        &game,
        &dir.path().join("p"),
        &NewDocument {
            name: "Moved".into(),
            ..Default::default()
        },
    )
    .unwrap();

    // Not while the edit is in the files of the game.
    ws.apply(&game, Default::default()).unwrap();
    drop(ws);
    let error = adopt_workspace(&game, &from, &project)
        .unwrap_err()
        .to_string();
    assert!(error.contains("put the originals back"), "{error}");
    assert!(Workspace::exists(&from) && !project.workspace_dir().unwrap().exists());
    let mut ws = Workspace::open(&from).unwrap();
    ws.unapply(&game, false).unwrap();
    drop(ws);

    let adopted = adopt_workspace(&game, &from, &project).unwrap();
    assert_eq!((adopted.changes, adopted.operations), (1, 1));
    assert!(!from.exists(), "the game's workspace is gone");
    let to = project.workspace_dir().unwrap();
    assert!(!to.join("applied.json").exists() && !to.join("lock").exists());
    // It is a project's from the move on: not written into the game by itself.
    assert!(
        std::fs::read_to_string(to.join("manifest.json"))
            .unwrap()
            .contains("\"owner\": \"project\"")
    );
    assert!(
        Workspace::open(&to)
            .unwrap()
            .apply(&game, Default::default())
            .is_err()
    );
    // The edit and its history are the project's: it is undone there.
    let mut ws = project.workspace(&game).unwrap();
    assert_eq!(ws.changes().count(), 1);
    assert!(ws.undo().unwrap().is_some());
    assert!(ws.is_empty());

    // A project that has edits takes no other workspace.
    ws.redo().unwrap();
    drop(ws);
    let other = dir.path().join("other-ws");
    let (_, second) = {
        let game_dir = dir.path().join("game");
        let game = GameInstance::open(&game_dir).unwrap();
        let mut ws = Workspace::for_game(&game, Some(&other)).unwrap();
        let stream = game.find_stream(testing::TEXT).unwrap();
        let mut table = StringTable::parse(&ws.current_data(&game, stream, 0).unwrap()).unwrap();
        table.strings[0][0] = "No".into();
        ws.set_string_table(&game, stream, 0, &table).unwrap();
        (game, ws)
    };
    drop(second);
    let error = adopt_workspace(&game, &other, &project)
        .unwrap_err()
        .to_string();
    assert!(error.contains("already has edits"), "{error}");
    assert!(Workspace::exists(&other));
    // A project of stage 36 (sources built into the game's workspace) has
    // nowhere to take it.
    let sources = fixture(Default::default());
    assert!(!sources.project.has_own_workspace());
    assert!(adopt_workspace(&game, &other, &sources.project).is_err());
}

#[test]
fn the_folder_of_a_workspace_is_stored_from_the_project_or_whole() {
    let root = std::env::current_dir().unwrap().join("p");
    assert_eq!(
        create::stored_workspace(&root, &root.join("workspace")),
        Path::new("workspace")
    );
    // Relative to where the command runs: not left as it is.
    let stored = create::stored_workspace(&root, Path::new("elsewhere/ws"));
    assert_eq!(
        stored,
        std::env::current_dir().unwrap().join("elsewhere/ws")
    );
    assert_eq!(
        create::stored_workspace(Path::new("p"), Path::new("p/ws")),
        Path::new("ws")
    );
}

/// Stage 107: the own workspace of a project is never written into the
/// game files by itself — whichever program records or opens it, by the
/// project or by its folder alone. The mark is in its manifest and comes
/// back from where the folder is. Other workspaces write as before.
#[test]
fn the_workspace_of_a_project_is_not_written_into_the_game_by_itself() {
    use crate::workspace::Owner;
    let dir = tempfile::tempdir().unwrap();
    let game_dir = dir.path().join("game");
    std::fs::create_dir_all(&game_dir).unwrap();
    testing::write_game(&game_dir).unwrap();
    let game = GameInstance::open(&game_dir).unwrap();
    let root = dir.path().join("p");
    let new = NewDocument {
        name: "P".into(),
        ..Default::default()
    };
    create_document(&game, &root, &new).unwrap();
    let ws_dir = root.join("workspace");
    assert_eq!(project_of_workspace(&ws_dir), Some(root.clone()));
    assert_eq!(project_of_workspace(&root), None);
    assert_eq!(
        project_of_workspace(&Workspace::default_dir(&game_dir)),
        None
    );
    let edit = |ws: &mut Workspace| {
        let stream = game.find_stream(testing::TEXT).unwrap();
        let mut table = StringTable::parse(&ws.current_data(&game, stream, 0).unwrap()).unwrap();
        table.strings[0][0] = "Si".into();
        ws.set_string_table(&game, stream, 0, &table).unwrap();
    };
    let pristine = std::fs::read(game_dir.join("gf2/text/abcd.str")).unwrap();
    let refused = |ws: &mut Workspace| {
        let error = ws.apply(&game, Default::default()).unwrap_err().to_string();
        assert!(
            error.contains("a project's own") && error.contains("project install"),
            "{error}"
        );
        assert_eq!(
            std::fs::read(game_dir.join("gf2/text/abcd.str")).unwrap(),
            pristine
        );
        assert!(!game_dir.join("_doctor_toolkit").exists());
    };

    // Recorded by a program that knows the folder only (`--workspace-dir`).
    let mut ws = Workspace::for_game(&game, Some(&ws_dir)).unwrap();
    assert_eq!(ws.owner(), Owner::Project);
    refused(&mut ws);
    edit(&mut ws);
    refused(&mut ws);
    drop(ws);
    let manifest = std::fs::read_to_string(ws_dir.join("manifest.json")).unwrap();
    assert!(manifest.contains("\"owner\": \"project\""), "{manifest}");
    // Opened again, for writing and for reading.
    refused(&mut Workspace::open(&ws_dir).unwrap());
    refused(&mut Workspace::open_read_only(&ws_dir).unwrap());
    // The originals can still be put back (nothing is in the game here).
    Workspace::open(&ws_dir)
        .unwrap()
        .unapply(&game, false)
        .unwrap();

    // The mark goes with the folder: a copy elsewhere is still a project's.
    let copy = dir.path().join("copy");
    std::fs::create_dir_all(&copy).unwrap();
    for name in ["manifest.json", "log.json"] {
        std::fs::copy(ws_dir.join(name), copy.join(name)).unwrap();
    }
    assert_eq!(Workspace::open(&copy).unwrap().owner(), Owner::Project);

    // A workspace written before the mark existed (or saved by a program
    // that does not know it): a project's where the project names it, an
    // ordinary one elsewhere.
    let unmarked = manifest.replace("  \"owner\": \"project\",\n", "");
    assert_ne!(unmarked, manifest);
    std::fs::write(ws_dir.join("manifest.json"), &unmarked).unwrap();
    std::fs::write(copy.join("manifest.json"), &unmarked).unwrap();
    refused(&mut Workspace::open(&ws_dir).unwrap());
    assert_eq!(Workspace::open(&copy).unwrap().owner(), Owner::Game);
    // Its next save writes the mark again.
    let mut ws = Workspace::open(&ws_dir).unwrap();
    ws.undo().unwrap();
    let manifest = std::fs::read_to_string(ws_dir.join("manifest.json")).unwrap();
    assert!(manifest.contains("\"owner\": \"project\""), "{manifest}");
    ws.redo().unwrap();

    // A project that keeps its workspace elsewhere marks it by opening it.
    let mut config = ProjectConfig::read(&root).unwrap();
    let far = dir.path().join("far/ws");
    config.build.workspace = Some(far.clone());
    config.write(&root).unwrap();
    assert_eq!(project_of_workspace(&ws_dir), None);
    let project = Project::open(&root).unwrap();
    let mut ws = project.workspace(&game).unwrap();
    assert_eq!(ws.owner(), Owner::Project);
    edit(&mut ws);
    drop(ws);
    refused(&mut Workspace::open(&far).unwrap());

    // The game's own workspace and one in a folder given to a command
    // write into the game as before.
    let other = dir.path().join("other");
    let mut ws = Workspace::for_game(&game, Some(&other)).unwrap();
    assert_eq!(ws.owner(), Owner::Game);
    edit(&mut ws);
    assert!(
        !std::fs::read_to_string(other.join("manifest.json"))
            .unwrap()
            .contains("owner")
    );
    ws.apply(&game, Default::default()).unwrap();
    assert_ne!(
        std::fs::read(game_dir.join("gf2/text/abcd.str")).unwrap(),
        pristine
    );
    ws.unapply(&game, false).unwrap();
    let mut ws = Workspace::for_game(&game, None).unwrap();
    assert_eq!(ws.owner(), Owner::Game);
    edit(&mut ws);
    ws.apply(&game, Default::default()).unwrap();
}

/// The payload files of the installed mod in `dir`.
fn installed_payloads(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    for prefix in std::fs::read_dir(dir.join("payloads")).unwrap() {
        for file in std::fs::read_dir(prefix.unwrap().path()).unwrap() {
            out.push(file.unwrap().path());
        }
    }
    out
}

/// Stage 107: a project goes into the game as a mod — its id, name and
/// version, enabled, last in the load order; installed again, it replaces
/// its earlier state and stays last.
#[test]
fn a_project_is_installed_into_the_mods() {
    use crate::mods::ModManager;
    let dir = tempfile::tempdir().unwrap();
    let game_dir = dir.path().join("game");
    std::fs::create_dir_all(&game_dir).unwrap();
    testing::write_game(&game_dir).unwrap();
    let game = GameInstance::open(&game_dir).unwrap();
    let root = dir.path().join("Yard Mod");
    let new = NewDocument {
        name: "Yard Mod".into(),
        author: "me".into(),
        ..Default::default()
    };
    let project = create_document(&game, &root, &new).unwrap();
    let mut mods = ModManager::open(&game).unwrap();
    // Nothing to install yet.
    let ws = project.workspace(&game).unwrap();
    assert!(
        project
            .install_into(&game, &mut mods, &ws, Default::default())
            .is_err()
    );
    assert!(mods.mods().is_empty());

    // Another mod is there first.
    let mut other = Workspace::for_game(&game, Some(&dir.path().join("other"))).unwrap();
    let level = game.find_stream(testing::LEVEL).unwrap();
    let mut table =
        StringTable::parse(&other.current_data(&game, level, testing::STRINGS).unwrap()).unwrap();
    table.strings[0][0] = "Other".into();
    other
        .set_string_table(&game, level, testing::STRINGS, &table)
        .unwrap();
    let info = crate::mods::ModInfo {
        id: "other".into(),
        name: "Other".into(),
        version: "1".into(),
        ..Default::default()
    };
    mods.install_workspace(&game, &other, &info, None).unwrap();

    let mut ws = project.workspace(&game).unwrap();
    let text = game.find_stream(testing::TEXT).unwrap();
    let mut table = StringTable::parse(&ws.current_data(&game, text, 0).unwrap()).unwrap();
    table.strings[0][0] = "Si".into();
    ws.set_string_table(&game, text, 0, &table).unwrap();
    let report = project
        .install_into(&game, &mut mods, &ws, Default::default())
        .unwrap();
    assert_eq!((report.id.as_str(), report.resources), ("yard-mod", 1));
    assert_eq!(report.replaced_version, None);
    let ids =
        |mods: &ModManager| -> Vec<String> { mods.mods().iter().map(|m| m.id.clone()).collect() };
    assert_eq!(ids(&mods), ["other", "yard-mod"]);
    let installed = mods.get("yard-mod").unwrap();
    assert!(installed.enabled);
    assert_eq!(installed.manifest.name, "Yard Mod");
    assert_eq!(installed.manifest.version, "1.0");
    assert_eq!(installed.manifest.author, "me");
    // The game is not written until the mods are.
    assert!(!mods.has_written());
    mods.apply(&game, Default::default()).unwrap();
    let game = GameInstance::open(&game_dir).unwrap();
    let written = crate::game::StreamFile::open(&game.streams()[text].path).unwrap();
    assert_eq!(
        StringTable::parse(&written.resource_data(0).unwrap())
            .unwrap()
            .strings[0][0],
        "Si"
    );

    // Edited and installed again: the same mod, updated, still last.
    mods.move_mod("yard-mod", 0).unwrap();
    table.strings[0][1] = "Non".into();
    ws.set_string_table(&game, text, 0, &table).unwrap();
    let report = project
        .install_into(&game, &mut mods, &ws, Default::default())
        .unwrap();
    assert_eq!(report.replaced_version.as_deref(), Some("1.0"));
    assert_eq!(ids(&mods), ["other", "yard-mod"]);
    // The mod is whole without the project: its data are files of its own.
    let at = game_dir.join("_doctor_toolkit/mods/yard-mod");
    let payloads = installed_payloads(&at);
    assert_eq!(payloads.len(), 1);
    assert!(std::fs::symlink_metadata(&payloads[0]).unwrap().is_file());
    // Installed once more without a change: the data it has is kept, not
    // copied again (the same file under the new folder).
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let before = std::fs::metadata(&payloads[0]).unwrap().ino();
        project
            .install_into(&game, &mut mods, &ws, Default::default())
            .unwrap();
        assert_eq!(installed_payloads(&at), payloads);
        assert_eq!(std::fs::metadata(&payloads[0]).unwrap().ino(), before);
    }
    // Another manager of the same game sees it.
    assert_eq!(
        ids(&ModManager::open(&game).unwrap()),
        ["other", "yard-mod"]
    );
}

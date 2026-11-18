//! Stage 36: a project of several streams of the game, built back.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::AtomicBool;
use std::time::Instant;

use doctor_toolkit::external;
use doctor_toolkit::game::{GameAssets, GameInstance};
use doctor_toolkit::import::model;
use doctor_toolkit::navigation::{GameIndex, unpack};
use doctor_toolkit::project::{self, BuildOptions, CreateOptions, EntryState, Kind};
use doctor_toolkit::resource::logic_import;
use doctor_toolkit::resource::pixels;
use doctor_toolkit::resource::texture::TexturePair;
use doctor_toolkit::scene::{self, ModelFormat};
use doctor_toolkit::workspace::{ResourceKey, Workspace};

/// Streams of different kinds: interface (textures, movies, fonts), text, a
/// level (SimGroup, models, collision, graphs), a cut-scene folder (shots,
/// animations, rigs, parts), a dialogue and two buildings with common
/// materials and textures.
const STREAMS: [&str; 7] = [
    "gf2/frontend",
    "gf2/text/dd7d7050",
    "mp_rooftops/mp_rooftops",
    "gf2/cuba/global/objectives/chapter60-mission08-killingcastro/cp_734/_734_igc_content",
    "gf2/globalfolder/dlg/03ccd0bc",
    "mp_rooftops/mp_rooftops/terrain/high/side2/scott_placeholder_building_001",
    "mp_rooftops/mp_rooftops/terrain/high/side2/scott_placeholder_building_002",
];

#[cfg(unix)]
#[test]
fn a_project_builds_only_what_changed_and_as_a_direct_import_would() {
    let Some(dir) = super::game_dir() else { return };
    let real = GameInstance::open(&dir).unwrap();
    let mut files: Vec<String> = STREAMS
        .iter()
        .map(|s| {
            real.streams()[real.find_stream(s).unwrap()]
                .rel_path
                .clone()
        })
        .collect();
    files.extend(real.tocs().iter().map(|t| t.rel_path.clone()));
    let refs: Vec<&str> = files.iter().map(String::as_str).collect();
    let originals: Vec<Vec<u8>> = refs
        .iter()
        .map(|rel| std::fs::read(dir.join(rel)).unwrap())
        .collect();
    let shadow = super::shadow_game(&dir, &refs);
    let game = GameInstance::open(shadow.path()).unwrap();
    let index = GameIndex::build(&game, &|_, _| {});
    let ws_dir = tempfile::tempdir().unwrap();
    let mut ws = Workspace::for_game(&game, Some(&ws_dir.path().join("ws"))).unwrap();
    let out = tempfile::tempdir().unwrap();
    let root = out.path().join("project");

    // Make the project.
    let started = Instant::now();
    let selection = unpack::select(&game, &[], &[]).unwrap();
    let total: usize = selection.iter().map(|(_, l)| l.len()).sum();
    let (p, report) = project::create(
        &game,
        &ws,
        Some(&index),
        &root,
        &CreateOptions {
            name: "Corpus project".into(),
            id: None,
            author: String::new(),
            selection,
            settings: Default::default(),
        },
        &|_, _| {},
        &AtomicBool::new(false),
    )
    .unwrap();
    let made = started.elapsed();
    eprintln!(
        "project: {} resource(s), {} entr(ies), {} shared, {} file(s), {:.1} MiB in {:.2} s; notes: {:#?}",
        report.resources,
        report.entries,
        report.shared,
        report.files,
        report.bytes as f64 / (1 << 20) as f64,
        made.as_secs_f64(),
        report.notes
    );
    assert_eq!(report.resources, total, "every resource is in the project");
    assert!(report.shared > 0, "the buildings share resources");
    let mut by_kind: BTreeMap<Kind, usize> = BTreeMap::new();
    for (_, entry, _) in p.entries() {
        *by_kind.entry(entry.kind).or_default() += 1;
    }
    eprintln!("entries by kind: {by_kind:?}");
    for kind in [
        Kind::Texture,
        Kind::Strings,
        Kind::Logic,
        Kind::Document,
        Kind::Model,
        Kind::Animations,
        Kind::View,
        Kind::Raw,
    ] {
        assert!(by_kind.get(&kind).is_some_and(|&n| n > 0), "{kind:?}");
    }
    // Conversions that fell back to raw data are few and named.
    assert!(report.notes.len() <= 5, "{:#?}", report.notes);
    let covered: BTreeSet<ResourceKey> = p
        .entries()
        .flat_map(|(_, e, _)| e.keys.iter().cloned())
        .collect();
    assert_eq!(covered.len(), total);

    // No edits: nothing to build, no workspace written.
    let started = Instant::now();
    let status = p.status(&ws).unwrap();
    let state_time = started.elapsed();
    assert_eq!(status.to_build(), 0);
    assert!(
        status.unknown_files.is_empty(),
        "{:?}",
        status.unknown_files
    );
    let started = Instant::now();
    let report = p.build(&game, &mut ws, &BuildOptions::default()).unwrap();
    let noop = started.elapsed();
    assert!(report.built.is_empty(), "{:?}", report.built);
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert!(!ws.is_saved());

    // Edit a texture, a string, a SimGroup and a model.
    let find = |kind: Kind, stream: &str, type_tag: &str| -> (String, project::Entry) {
        let mut found: Vec<(String, project::Entry)> = p
            .entries()
            .filter(|(_, e, d)| {
                e.kind == kind && e.type_tag == type_tag && *d == format!("src/{stream}.str")
            })
            .map(|(id, e, _)| (id, e.clone()))
            .collect();
        found.sort_by(|a, b| a.0.cmp(&b.0));
        found
            .into_iter()
            .next()
            .unwrap_or_else(|| panic!("{kind:?} in {stream}"))
    };
    let (texture_id, texture) = p
        .entries()
        .find(|(id, e, _)| e.kind == Kind::Texture && id.ends_with("/buttons.png"))
        .map(|(id, e, _)| (id, e.clone()))
        .expect("the buttons texture");
    let png_path = root.join(&texture_id);
    let (w, h, mut rgba) = pixels::decode_png(&std::fs::read(&png_path).unwrap()).unwrap();
    for px in rgba.chunks_mut(4) {
        px[1] = 255 - px[1];
    }
    let png = pixels::encode_png(w, h, &rgba).unwrap();
    std::fs::write(&png_path, &png).unwrap();

    let (strings_id, strings) = find(Kind::Strings, "gf2/text/dd7d7050", "lh2");
    let mut strings_doc: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.join(&strings_id)).unwrap()).unwrap();
    let column = strings_doc["languages"][0].as_str().unwrap().to_owned();
    strings_doc["entries"][3][&column] = "doctor_toolkit project test".into();
    std::fs::write(
        root.join(&strings_id),
        serde_json::to_vec_pretty(&strings_doc).unwrap(),
    )
    .unwrap();

    let (group_id, group) = find(Kind::Logic, "mp_rooftops/mp_rooftops", "simgroup");
    let mut group_doc: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.join(&group_id)).unwrap()).unwrap();
    let object = group_doc["objects"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|o| o["position"].is_array())
        .expect("an object with a position");
    let x = object["position"][0].as_f64().unwrap();
    object["position"][0] = (x + 1.5).into();
    std::fs::write(
        root.join(&group_id),
        serde_json::to_vec_pretty(&group_doc).unwrap(),
    )
    .unwrap();

    let (model_id, model_entry) = find(Kind::Model, "mp_rooftops/mp_rooftops", "mesh");
    let mut model_scene = scene::read_file(&root.join(&model_id)).unwrap();
    for part in &mut model_scene.parts {
        for v in &mut part.geometry.positions {
            v[1] += 0.25;
        }
    }
    let glb = scene::to_bytes(&model_scene, ModelFormat::Glb).unwrap();
    std::fs::write(root.join(&model_id), &glb).unwrap();

    let status = p.status(&ws).unwrap();
    let mut modified: Vec<&str> = status
        .entries
        .iter()
        .filter(|e| e.state == EntryState::Modified)
        .map(|e| e.id.as_str())
        .collect();
    modified.sort();
    let mut expected = vec![
        texture_id.as_str(),
        strings_id.as_str(),
        group_id.as_str(),
        model_id.as_str(),
    ];
    expected.sort();
    assert_eq!(modified, expected);

    let started = Instant::now();
    let report = p.build(&game, &mut ws, &BuildOptions::default()).unwrap();
    let incremental = started.elapsed();
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    let mut built: Vec<&str> = report.built.iter().map(|b| b.id.as_str()).collect();
    built.sort();
    assert_eq!(built, expected);
    assert_eq!(ws.head(), 1, "one undoable operation");
    let project_changes: BTreeMap<ResourceKey, String> = ws
        .changes()
        .map(|c| (c.key.clone(), c.payload.sha256.clone()))
        .collect();
    // Only the edited resources (and what the model import adds) changed.
    let edited: BTreeSet<&ResourceKey> = texture
        .keys
        .iter()
        .chain(&strings.keys)
        .chain(&group.keys)
        .chain(&model_entry.keys)
        .collect();
    for key in project_changes.keys() {
        let added = ws.change(key).unwrap().is_added();
        assert!(
            edited.contains(key) || added || key.type_id == doctor_toolkit::hash::sdbm(b"material"),
            "{key} changed"
        );
    }

    // The same files imported directly give the same data.
    let mut direct = Workspace::for_game(&game, Some(&ws_dir.path().join("direct"))).unwrap();
    let locate = |key: &ResourceKey| {
        let s = game.find_stream(&key.stream).unwrap();
        (
            s,
            key.find_in(game.open_stream(s).unwrap().layout()).unwrap(),
        )
    };
    let (s, header) = locate(&texture.keys[0]);
    let (_, data) = locate(&texture.keys[1]);
    direct
        .import_texture(&game, s, TexturePair { header, data }, &png, true)
        .unwrap();
    // As "Open in external editor" imports a saved JSON file.
    let target =
        external::Target::find(&game, &strings.keys[0], Some(external::Format::Json)).unwrap();
    external::import(
        &game,
        &mut direct,
        &target,
        &serde_json::to_vec_pretty(&strings_doc).unwrap(),
        None,
        &Default::default(),
    )
    .unwrap();
    let (s, r) = locate(&group.keys[0]);
    let imported = logic_import::apply_json(
        group.keys[0].type_id,
        &game.open_stream(s).unwrap().resource_data(r).unwrap(),
        &group_doc,
    )
    .unwrap();
    direct
        .set_resource(&game, s, r, imported.data, "logic-import")
        .unwrap();
    let (s, r) = locate(&model_entry.keys[0]);
    let arc = std::sync::Arc::new(game.clone());
    let (resources, _) = game.build_index(|_| {});
    let mut assets = GameAssets::new(arc, &resources);
    let read_back = scene::read_file(&root.join(&model_id)).unwrap();
    let plan = model::plan(
        &game,
        &direct,
        &resources,
        &mut assets,
        s,
        r,
        &read_back,
        &model::ImportOptions::default(),
    )
    .unwrap();
    plan.commit(&mut direct, &game).unwrap();
    let direct_changes: BTreeMap<ResourceKey, String> = direct
        .changes()
        .map(|c| (c.key.clone(), c.payload.sha256.clone()))
        .collect();
    assert_eq!(project_changes, direct_changes);

    // Building again does nothing; the edited entries are built.
    let report = p.build(&game, &mut ws, &BuildOptions::default()).unwrap();
    assert!(report.built.is_empty(), "{:?}", report.built);
    let status = p.status(&ws).unwrap();
    assert_eq!(status.count(&EntryState::Built), 4);

    // A mod of the project: what the build changed.
    let mod_file = out.path().join("project.zip");
    let mod_report = p.build_mod(&ws, &mod_file, None).unwrap();
    assert_eq!(mod_report.resources, project_changes.len());

    // Every entry built again (textures and models encoded anew).
    let started = Instant::now();
    let full = p
        .build(
            &game,
            &mut ws,
            &BuildOptions {
                force: true,
                ..BuildOptions::default()
            },
        )
        .unwrap();
    let full_time = started.elapsed();
    // Which kinds build back to the same data from untouched files.
    let mut same: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    for built in &full.built {
        let (entry, _) = p.entry(&built.id).unwrap();
        let slot = same
            .entry(format!("{:?} {}", entry.kind, entry.type_tag))
            .or_default();
        slot.0 += 1;
        if built.changed.is_empty() {
            slot.1 += 1;
        }
    }
    eprintln!("forced build, (entries, unchanged data) by kind and type: {same:#?}");
    eprintln!(
        "build: status {:.2} s, nothing to build {:.2} s, 4 edited files {:.2} s, \
         all {} entries {:.2} s ({} error(s): {:?})",
        state_time.as_secs_f64(),
        noop.as_secs_f64(),
        incremental.as_secs_f64(),
        full.built.len() + full.errors.len(),
        full_time.as_secs_f64(),
        full.errors.len(),
        full.errors.iter().take(5).collect::<Vec<_>>()
    );
    assert!(
        full.errors.len() <= 3,
        "forced build errors: {:#?}",
        full.errors
    );

    // The game files are untouched.
    for (rel, original) in refs.iter().zip(&originals) {
        assert_eq!(&std::fs::read(dir.join(rel)).unwrap(), original, "{rel}");
    }
}

/// Regular files below `dir` (links are not counted): how many, and their bytes.
#[cfg(unix)]
/// The sources of the project in `from` (and the data they are applied to)
/// in the folder `to`: what version control holds — no workspace, no cache.
fn checkout(from: &std::path::Path, to: &std::path::Path) -> project::Project {
    fn copy(from: &std::path::Path, to: &std::path::Path) {
        std::fs::create_dir_all(to).unwrap();
        for entry in std::fs::read_dir(from).unwrap() {
            let entry = entry.unwrap();
            let name = entry.file_name();
            let (source, target) = (entry.path(), to.join(&name));
            if source.is_dir() {
                if name != "workspace" {
                    copy(&source, &target);
                }
            } else if name != "build.json" {
                std::fs::copy(&source, &target).unwrap();
            }
        }
    }
    copy(from, to);
    project::Project::open(to).unwrap()
}

/// Stage 112: a workspace with what a mod makes anew on a real level — a
/// zone (a new stream with a SimGroup), a model of the level imported into
/// it as a new set, a texture cloned into it, a resource added to a stream
/// of the game — next to a replaced string: its sources build the same
/// workspace in an empty one, key for key and byte for byte, and so do the
/// sources of that one; an edited model and an edited picture are encoded
/// again into the same resources.
#[cfg(unix)]
#[test]
fn the_sources_of_a_workspace_with_a_new_zone_build_it_again() {
    use doctor_toolkit::resource::lh2::StringTable;
    use doctor_toolkit::resource::mesh;
    use doctor_toolkit::resource::simgroup::Guid;
    use doctor_toolkit::workspace::{Change, NewStream};
    use doctor_toolkit::zone_ops;
    use std::sync::Arc;

    const LEVEL: &str = "mp_rooftops/mp_rooftops";
    const BUILDING: &str =
        "mp_rooftops/mp_rooftops/terrain/high/side2/scott_placeholder_building_001";
    const TEXT: &str = "gf2/text/dd7d7050";
    const ZONE: &str = "mp_rooftops/mp_rooftops/doctor_zone";
    let Some(dir) = super::game_dir() else { return };
    let real = GameInstance::open(&dir).unwrap();
    let mut files: Vec<String> = [LEVEL, BUILDING, TEXT]
        .iter()
        .map(|s| {
            real.streams()[real.find_stream(s).unwrap()]
                .rel_path
                .clone()
        })
        .collect();
    files.extend(real.tocs().iter().map(|t| t.rel_path.clone()));
    let refs: Vec<&str> = files.iter().map(String::as_str).collect();
    let shadow = super::shadow_game(&dir, &refs);
    let game = GameInstance::open(shadow.path()).unwrap();
    let index = GameIndex::build(&game, &|_, _| {});
    let resources = game.build_index(|_| {}).0;
    let out = tempfile::tempdir().unwrap();
    let root = out.path().join("project");
    let new = project::NewDocument {
        name: "New zone".into(),
        ..Default::default()
    };
    let mut p = project::create_document(&game, &root, &new).unwrap();
    let mut ws = p.workspace(&game).unwrap();
    let position = |stream: usize, tag: &[u8]| {
        let type_id = doctor_toolkit::hash::sdbm(tag);
        game.open_stream(stream)
            .unwrap()
            .layout()
            .resources
            .iter()
            .position(|r| r.header.type_id == type_id)
            .unwrap()
    };

    // A replaced string.
    let text = game.find_stream(TEXT).unwrap();
    let table_at = position(text, b"lh2");
    let mut table = StringTable::parse(&ws.current_data(&game, text, table_at).unwrap()).unwrap();
    table.strings[0][0] = "Doctor".into();
    ws.set_string_table(&game, text, table_at, &table).unwrap();
    // A zone: a new stream with a SimGroup.
    let zone = zone_ops::add_stream(
        &game,
        &mut ws,
        &zone_ops::NewStreamParams {
            name: ZONE.into(),
            parent: LEVEL.into(),
            ..Default::default()
        },
    )
    .unwrap();
    zone_ops::add_simgroup(
        &game,
        &resources,
        &mut ws,
        &zone_ops::NewGroupParams {
            stream: zone.name.clone(),
            ..Default::default()
        },
        &|_: &Guid| false,
    )
    .unwrap();
    // A texture of a building cloned into it, and into the building itself.
    let building = game.find_stream(BUILDING).unwrap();
    let texture_at = position(building, b"tg4h");
    for to in [Some(zone.name.clone()), None] {
        zone_ops::clone_resources(
            &game,
            &resources,
            &mut ws,
            &zone_ops::CloneParams {
                stream: BUILDING.into(),
                indices: vec![texture_at],
                to,
                ..Default::default()
            },
        )
        .unwrap();
    }
    // A model of the level, moved up, as a new set in the zone.
    let level = game.find_stream(LEVEL).unwrap();
    let mesh_at = position(level, b"mesh");
    let mut assets = GameAssets::new(Arc::new(game.clone()), &resources);
    let model = mesh::load(&game.open_stream(level).unwrap(), mesh_at).unwrap();
    let mut scene_of = scene::game::model_scene(&model, &mut assets).unwrap();
    for part in &mut scene_of.parts {
        for v in &mut part.geometry.positions {
            v[1] += 0.5;
        }
    }
    let glb = scene::to_bytes(&scene_of, ModelFormat::Glb).unwrap();
    let parsed = doctor_toolkit::import::gltf::read(&glb, None).unwrap();
    let options = model::ImportOptions {
        new_name: Some("doctor_prop".into()),
        ..Default::default()
    };
    let mut plan = model::plan(
        &game,
        &ws,
        &resources,
        &mut assets,
        level,
        mesh_at,
        &parsed,
        &options,
    )
    .unwrap();
    plan.put_into(&game, &ws, &zone.name).unwrap();
    plan.commit(&mut ws, &game).unwrap();

    let state = |ws: &Workspace| -> (BTreeMap<String, Change>, Vec<NewStream>) {
        let changes = ws
            .changes()
            .map(|c| {
                let mut change = c.clone();
                change.label.clear();
                (c.key.to_string(), change)
            })
            .collect();
        let streams = ws
            .new_streams()
            .map(|s| NewStream {
                label: String::new(),
                ..s.clone()
            })
            .collect();
        (changes, streams)
    };
    let (changes, streams) = state(&ws);
    let added = changes.values().filter(|c| c.is_added()).count();
    assert!(added >= 7, "{:?}", changes.keys().collect::<Vec<_>>());

    // Sources of all of it.
    let started = Instant::now();
    let report = p.export_workspace(&game, &ws, Some(&index)).unwrap();
    let exported = started.elapsed();
    assert!(report.notes.is_empty(), "{:#?}", report.notes);
    let covered: BTreeSet<String> = p
        .entries()
        .flat_map(|(_, e, _)| e.keys.iter().map(ToString::to_string).collect::<Vec<_>>())
        .collect();
    assert_eq!(
        covered.iter().collect::<Vec<_>>(),
        changes.keys().collect::<Vec<_>>()
    );
    let mut by_kind: BTreeMap<Kind, usize> = BTreeMap::new();
    for (_, entry, _) in p.entries() {
        *by_kind.entry(entry.kind).or_default() += 1;
    }
    for kind in [Kind::Model, Kind::Texture, Kind::Logic, Kind::Strings] {
        assert!(by_kind.get(&kind).is_some_and(|&n| n > 0), "{by_kind:?}");
    }
    assert_eq!(p.status(&ws).unwrap().to_build(), 0);

    // A checkout of the sources builds the workspace again.
    let second = checkout(&root, &out.path().join("second"));
    let mut ws2 = second.workspace(&game).unwrap();
    let started = Instant::now();
    let report = second
        .build(&game, &mut ws2, &BuildOptions::default())
        .unwrap();
    let built = started.elapsed();
    assert!(report.errors.is_empty(), "{:#?}", report.errors);
    assert_eq!(ws2.head(), 1, "one undoable operation");
    let (changes2, streams2) = state(&ws2);
    assert_eq!(streams2, streams);
    assert_eq!(
        changes2.keys().collect::<Vec<_>>(),
        changes.keys().collect::<Vec<_>>()
    );
    for (key, change) in &changes {
        assert_eq!(&changes2[key], change, "{key}");
    }
    assert_eq!(second.status(&ws2).unwrap().to_build(), 0);

    // The second round: the sources of the built workspace build it byte
    // for byte.
    checkout(&out.path().join("second"), &out.path().join("fresh"));
    std::fs::remove_dir_all(out.path().join("fresh/src")).unwrap();
    let mut fresh = project::Project::open(out.path().join("fresh")).unwrap();
    let report = fresh.export_workspace(&game, &ws2, Some(&index)).unwrap();
    assert!(report.notes.is_empty(), "{:#?}", report.notes);
    let third = checkout(&out.path().join("fresh"), &out.path().join("third"));
    let mut ws3 = third.workspace(&game).unwrap();
    let report = third
        .build(&game, &mut ws3, &BuildOptions::default())
        .unwrap();
    assert!(report.errors.is_empty(), "{:#?}", report.errors);
    let (changes3, streams3) = state(&ws3);
    assert_eq!(streams3, streams2);
    let differ: Vec<&String> = changes2
        .iter()
        .filter(|(key, change)| changes3.get(*key) != Some(change))
        .map(|(key, _)| key)
        .collect();
    assert!(differ.is_empty(), "second round: {differ:?}");

    // The model and a picture are edited: encoded again, into the same
    // resources and nothing else. (A picture decoded and encoded again is
    // other data: DXT does not give its own blocks back.)
    let find = |kind: Kind| {
        third
            .entries()
            .find(|(_, e, d)| e.kind == kind && e.is_added() && d.contains("doctor_zone"))
            .map(|(id, e, _)| (id, e.clone()))
            .unwrap()
    };
    let (model_id, model_entry) = find(Kind::Model);
    let mut edited = scene::read_file(&third.root().join(&model_id)).unwrap();
    for part in &mut edited.parts {
        for v in &mut part.geometry.positions {
            v[0] += 1.0;
        }
    }
    std::fs::write(
        third.root().join(&model_id),
        scene::to_bytes(&edited, ModelFormat::Glb).unwrap(),
    )
    .unwrap();
    let (texture_id, texture_entry) = find(Kind::Texture);
    let png = std::fs::read(third.root().join(&texture_id)).unwrap();
    let (w, h, mut rgba) = pixels::decode_png(&png).unwrap();
    for px in rgba.chunks_mut(4) {
        px[1] = 255 - px[1];
    }
    std::fs::write(
        third.root().join(&texture_id),
        pixels::encode_png(w, h, &rgba).unwrap(),
    )
    .unwrap();
    let report = third
        .build(&game, &mut ws3, &BuildOptions::default())
        .unwrap();
    assert!(report.errors.is_empty(), "{:#?}", report.errors);
    let (changes4, _) = state(&ws3);
    assert_eq!(
        changes4.keys().collect::<Vec<_>>(),
        changes.keys().collect::<Vec<_>>()
    );
    let changed: Vec<&String> = changes4
        .iter()
        .filter(|(key, change)| changes3.get(*key) != Some(change))
        .map(|(key, _)| key)
        .collect();
    let mesh_key = model_entry.keys[0].to_string();
    assert!(changed.contains(&&mesh_key), "{changed:?}");
    assert!(
        changed.contains(&&texture_entry.keys[1].to_string()),
        "{changed:?}"
    );
    assert!(
        changed.iter().all(|key| model_entry
            .keys
            .iter()
            .chain(&texture_entry.keys)
            .any(|k| k.to_string() == **key)),
        "{changed:?}"
    );
    let moved = mesh::Model::parse(
        &ws3.data(&model_entry.keys[0]).unwrap().unwrap(),
        &ws3.data(&model_entry.keys[1]).unwrap().unwrap(),
    )
    .unwrap();
    let was = mesh::Model::parse(
        &ws2.data(&model_entry.keys[0]).unwrap().unwrap(),
        &ws2.data(&model_entry.keys[1]).unwrap().unwrap(),
    )
    .unwrap();
    assert_eq!(moved.name, "doctor_prop");
    assert!((moved.aabb_min[0] - was.aabb_min[0] - 1.0).abs() < 1e-3);
    assert_eq!(moved.records.len(), was.records.len());
    eprintln!(
        "{} change(s), {added} added; {} entries ({by_kind:?}); sources in {:.2} s, build into \
         an empty workspace in {:.2} s; after the edit of a model and a picture changed: \
         {changed:?}",
        changes.len(),
        p.entries().count(),
        exported.as_secs_f64(),
        built.as_secs_f64(),
    );
}

fn files_below(dir: &std::path::Path) -> (usize, u64) {
    let mut out = (0, 0);
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let Ok(meta) = std::fs::symlink_metadata(entry.path()) else {
            continue;
        };
        if meta.is_dir() {
            let (files, bytes) = files_below(&entry.path());
            out = (out.0 + files, out.1 + bytes);
        } else if meta.is_file() {
            out = (out.0 + 1, out.1 + meta.len());
        }
    }
    out
}

/// Stage 107, a measurement: a large project — the largest resource of
/// every stream below `gf2/cuba/` changed (`DOCTOR_MEASURE_STREAMS` names
/// another prefix) — goes into the game as a mod. How long the staged copy
/// of a launch takes with the project as the last mod, against the
/// workspace alone (the way before stage 107); and how long Install into
/// Mods takes, the first time and again. The numbers are in
/// `docs/project.md` §0.
#[cfg(unix)]
#[test]
fn a_large_project_is_installed_as_a_mod() {
    use doctor_toolkit::game::StreamFile;
    use doctor_toolkit::mods::ModManager;
    use doctor_toolkit::play::overlay::{Overlay, Source, Streams};
    use doctor_toolkit::workspace::{ResourceEdit, StorageChoice};

    let Some(dir) = super::game_dir() else { return };
    let real = GameInstance::open(&dir).unwrap();
    let prefix = std::env::var("DOCTOR_MEASURE_STREAMS").unwrap_or_else(|_| "gf2/cuba/".into());
    let mut files: Vec<String> = real
        .streams()
        .iter()
        .filter(|s| s.stream_name().starts_with(&prefix))
        .map(|s| s.rel_path.clone())
        .collect();
    let stream_bytes: u64 = files
        .iter()
        .map(|rel| std::fs::metadata(dir.join(rel)).unwrap().len())
        .sum();
    files.extend(real.tocs().iter().map(|t| t.rel_path.clone()));
    files.push("godfather2.exe".into());
    let refs: Vec<&str> = files.iter().map(String::as_str).collect();
    let shadow = super::shadow_game(&dir, &refs);
    let game = GameInstance::open(shadow.path()).unwrap();

    let out = tempfile::tempdir().unwrap();
    let root = out.path().join("project");
    let new = project::NewDocument {
        name: "Measured".into(),
        ..Default::default()
    };
    let p = project::create_document(&game, &root, &new).unwrap();
    let edits: Vec<(usize, Vec<ResourceEdit>)> = game
        .streams()
        .iter()
        .enumerate()
        .filter_map(|(at, file)| {
            let stream = StreamFile::open(&file.path).ok()?;
            let (index, _) = stream
                .layout()
                .resources
                .iter()
                .enumerate()
                .max_by_key(|(_, r)| r.header.data_size)?;
            let mut data = stream.resource_data(index).ok()?;
            *data.first_mut()? ^= 0xff;
            Some((
                at,
                vec![ResourceEdit {
                    index,
                    data,
                    storage: StorageChoice::Keep,
                }],
            ))
        })
        .collect();
    let payload: u64 = edits.iter().map(|(_, e)| e[0].data.len() as u64).sum();
    let mib = |bytes: u64| bytes as f64 / (1 << 20) as f64;
    eprintln!(
        "{} stream(s) below {prefix} ({:.1} MiB of stream files), one resource changed in \
         each: {:.1} MiB of new data",
        edits.len(),
        mib(stream_bytes),
        mib(payload)
    );
    let mut ws = p.workspace(&game).unwrap();
    ws.change_resources(&game, "measured", &edits, &[]).unwrap();
    // The same changes in a workspace that is nobody's project.
    let plain = out.path().join("plain");
    Workspace::for_game(&game, Some(&plain))
        .unwrap()
        .change_resources(&game, "measured", &edits, &[])
        .unwrap();
    drop(edits);

    let staged = out.path().join("staged");
    let mut redirects = Vec::new();
    for (name, source) in [
        ("the workspace alone", Source::Workspace(plain.clone())),
        ("the project as a mod", Source::Project(root.clone())),
    ] {
        for streams in [Streams::Rebuilt, Streams::Patched] {
            let mut times = Vec::new();
            let mut size = (0, 0);
            for _ in 0..3 {
                let started = Instant::now();
                let overlay =
                    Overlay::build_with(&game, Some(&source), None, &staged, streams).unwrap();
                times.push(started.elapsed().as_secs_f64());
                size = files_below(&staged);
                redirects.push(overlay.redirects().len() + overlay.resources().len());
                overlay.remove().unwrap();
            }
            eprintln!(
                "staged copy, {name}, {}: {:.2}–{:.2} s; {:.1} MiB in {} file(s)",
                streams.name(),
                times.iter().copied().fold(f64::MAX, f64::min),
                times.iter().copied().fold(0.0, f64::max),
                mib(size.1),
                size.0
            );
        }
    }
    assert!(redirects.windows(2).all(|w| w[0] == w[1]), "{redirects:?}");

    // Install into Mods: the first time, again unchanged, again after one
    // more edit.
    let mut mods = ModManager::open(&game).unwrap();
    let time = |what: &str, ws: &Workspace, mods: &mut ModManager| {
        let started = Instant::now();
        let report = p.install_into(&game, mods, ws, Default::default()).unwrap();
        let (count, bytes) = files_below(&mods.dir().join(&report.id));
        eprintln!(
            "install into Mods, {what}: {:.2} s; {} resource(s), {:.1} MiB in {count} file(s)",
            started.elapsed().as_secs_f64(),
            report.resources,
            mib(bytes),
        );
    };
    time("the first time", &ws, &mut mods);
    time("again, nothing changed", &ws, &mut mods);
    let file = &game.streams()[0];
    let stream = StreamFile::open(&file.path).unwrap();
    let mut data = stream.resource_data(0).unwrap();
    data.push(0);
    ws.set_resources(
        &game,
        "one more",
        0,
        &[ResourceEdit {
            index: 0,
            data,
            storage: StorageChoice::Keep,
        }],
    )
    .unwrap();
    time("again, one more resource changed", &ws, &mut mods);
}

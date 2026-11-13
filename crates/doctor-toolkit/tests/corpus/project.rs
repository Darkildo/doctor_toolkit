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

//! The shadow meshes of the game (`*__shadow_dmg0` of the vehicles,
//! `shadowfemale_lod1`, `shadowmale_lod1`) as a project: the meshes without
//! a `meshvolatile`, most of them the same in several streams and so
//! entries of `shared/`
//! (`docs/analysis/whole-game-project.md` §3.2). A forced build writes
//! every resource from one entry only: the copies of a mesh in other
//! streams that entries of the project build are not replaced by the
//! import of another entry.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::AtomicBool;

use doctor_toolkit::game::GameInstance;
use doctor_toolkit::navigation::{GameIndex, unpack};
use doctor_toolkit::project::{self, BuildOptions, CreateOptions, Kind};
use doctor_toolkit::resource::mesh::{TYPE_MESH, TYPE_MESH_VOLATILE};
use doctor_toolkit::workspace::{ResourceKey, Workspace};

#[test]
fn shared_shadow_meshes_build_each_resource_once() {
    let Some(dir) = super::game_dir() else { return };
    let game = GameInstance::open(&dir).unwrap();
    let index = GameIndex::build(&game, &|_, _| {});
    // The meshes without a meshvolatile (`!ids`) in their stream: the
    // shadows of the cars and of the characters.
    let records: Vec<_> = index
        .ids()
        .filter(|&id| {
            let record = index.record(id);
            let mut volatile = index.key(id);
            volatile.type_id = TYPE_MESH_VOLATILE;
            volatile.ids = volatile.ids.map(|i| !i);
            record.header.type_id == TYPE_MESH && index.find_key(&volatile).is_none()
        })
        .collect();
    for &id in &records {
        let name = index.record(id).name().unwrap_or_default();
        assert!(
            name.ends_with("__shadow_dmg0") || name.starts_with("shadow"),
            "{name}"
        );
    }
    assert_eq!(records.len(), 50);
    let selection = unpack::select_records(&index, &records);
    let total: usize = selection.iter().map(|(_, l)| l.len()).sum();
    // The game file is only read: the workspace is a folder of its own.
    let out = tempfile::tempdir().unwrap();
    let mut ws = Workspace::for_game(&game, Some(&out.path().join("ws"))).unwrap();
    let (p, report) = project::create(
        &game,
        &ws,
        Some(&index),
        &out.path().join("project"),
        &CreateOptions {
            name: "Shadow meshes".into(),
            id: None,
            author: String::new(),
            selection,
            settings: Default::default(),
        },
        &|_, _| {},
        &AtomicBool::new(false),
    )
    .unwrap();
    eprintln!(
        "{total} shadow mesh(es) in {} stream(s): {} entr(ies), {} shared",
        records
            .iter()
            .map(|&id| index.stream_of(id))
            .collect::<BTreeSet<_>>()
            .len(),
        report.entries,
        report.shared
    );
    assert_eq!(report.resources, total);
    assert!(report.notes.is_empty(), "{:#?}", report.notes);
    assert!(report.shared >= 10, "{report:?}");
    // No meshvolatile in the game: an entry is the mesh alone, one key per
    // stream.
    let mut keys_of: BTreeMap<String, Vec<ResourceKey>> = BTreeMap::new();
    for (id, entry, _) in p.entries() {
        assert_eq!(entry.kind, Kind::Model, "{id}");
        assert!(entry.keys.iter().all(|k| k.type_id == TYPE_MESH), "{id}");
        keys_of.insert(id, entry.keys.clone());
    }

    // Unchanged: nothing to build.
    let nothing = p.build(&game, &mut ws, &BuildOptions::default()).unwrap();
    assert!(
        nothing.built.is_empty() && nothing.errors.is_empty(),
        "{nothing:?}"
    );

    // Forced: every entry is built, each resource by one entry, in the
    // streams of its own keys.
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
    assert!(full.errors.is_empty(), "{:#?}", full.errors);
    assert_eq!(full.built.len(), keys_of.len());
    let mut writers: BTreeMap<ResourceKey, Vec<&str>> = BTreeMap::new();
    for built in &full.built {
        let keys = &keys_of[&built.id];
        for key in &built.changed {
            writers.entry(key.clone()).or_default().push(&built.id);
            // The mesh of a key, or the meshvolatile the build adds to it
            // (the mesh builder keeps the second vertex stream there).
            let own = keys.contains(key)
                || (key.type_id == TYPE_MESH_VOLATILE
                    && keys
                        .iter()
                        .any(|k| k.stream == key.stream && k.ids.map(|i| !i) == key.ids));
            assert!(own, "{}: {key} is not of the entry", built.id);
        }
        for key in keys {
            assert!(built.changed.contains(key), "{}: {key} not built", built.id);
        }
    }
    let twice: Vec<_> = writers.iter().filter(|(_, w)| w.len() > 1).collect();
    assert!(twice.is_empty(), "{twice:#?}");
    let meshes = writers.keys().filter(|k| k.type_id == TYPE_MESH).count();
    assert_eq!(meshes, total, "every mesh is built");
}

//! Stage 112: what a mod makes that the game does not have — resources it
//! adds, streams it creates, a model imported as a new set — is in the
//! sources of a project, and a build makes it again in another workspace.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use super::*;
use crate::game::GameAssets;
use crate::resource::lh2::StringTable;
use crate::resource::simgroup::Guid;
use crate::testing as mini;
use crate::workspace::{Change, NewResourceEdit, NewStream, StorageChoice};
use crate::zone_ops::{self, StreamLoad};

/// The stream the mod of these tests creates.
const ZONE: &str = "gf2/town/doctor";

/// A stream of props added to the town of the miniature game: a barrel
/// with its mesh, the volatile data right after it (as the game keeps them,
/// so that a clone of the mesh takes it along), collision and a metamodel
/// ([`PROPS_MODEL`]).
const PROPS: &str = "gf2/town/props";
const PROPS_MODEL: usize = 3;

/// The miniature game with its town and [`PROPS`] in `dir/game`.
fn town(dir: &Path) -> GameInstance {
    use crate::container::ustream::{
        FileHeader, ResourceHeader, Storage, StreamLayout, StreamWriter,
    };
    use crate::resource::hkx::collision::{self, CollisionKind, CollisionMesh};
    let root = dir.join("game");
    std::fs::create_dir_all(&root).unwrap();
    mini::write_game(&root).unwrap();
    mini::write_town(&root).unwrap();

    let header = |tag: &[u8], id: u32| ResourceHeader {
        mem_policy: crate::hash::sdbm(b"default"),
        alignment: 0x10,
        type_id: crate::hash::sdbm(tag),
        ids: [id; 4],
        data_size: 0,
    };
    let (mesh, volatile) = mini::quad_mesh("barrel", mini::MATERIAL_NAME).unwrap();
    assert!(!volatile.is_empty());
    let mut shape = CollisionMesh::default();
    shape.append(
        &[
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, 0.0, 1.0],
        ],
        &[[0, 1, 2], [0, 2, 3], [0, 3, 1], [1, 3, 2]],
    );
    let (hkx, _) = collision::build(
        &shape,
        CollisionKind::Box,
        collision::DEFAULT_MATERIAL,
        "barrel",
    )
    .unwrap();
    let (mesh_id, hkx_id, model_id) = (0x7002_0002, 0x7002_0003, 0x7002_0001);
    let mut props = StreamWriter::new(&FileHeader::GF2_PC, crate::io::Endian::Little);
    props
        .push_resource(&header(b"mesh", mesh_id), &mesh, Storage::RefPack)
        .unwrap();
    props
        .push_resource(&header(b"meshvolatile", !mesh_id), &volatile, Storage::Raw)
        .unwrap();
    props
        .push_resource(&header(b"hkx", hkx_id), &hkx, Storage::RefPack)
        .unwrap();
    props
        .push_resource(
            &header(b"metamodel", model_id),
            &mini::prop_metamodel(model_id, mesh_id, Some(hkx_id)),
            Storage::RefPack,
        )
        .unwrap();
    std::fs::write(root.join(format!("{PROPS}.str")), props.finish()).unwrap();
    let mut listed = Vec::new();
    for name in [mini::LEVEL, mini::TEXT, mini::TOWN, mini::YARD, PROPS] {
        let data = std::fs::read(root.join(format!("{name}.str"))).unwrap();
        listed.push((name, StreamLayout::parse(&data).unwrap().total_data_size()));
    }
    let table = crate::toc::Toc::parse(&mini::toc_bytes(&listed)).unwrap();
    std::fs::write(root.join("gf2/level.toc"), table.to_bytes().unwrap()).unwrap();
    GameInstance::open(&root).unwrap()
}

fn document(game: &GameInstance, root: &Path) -> Project {
    let new = NewDocument {
        name: "Yard Mod".into(),
        ..Default::default()
    };
    create_document(game, root, &new).unwrap()
}

/// A raw resource for [`Workspace::add_resources`].
fn raw(stream: &str, tag: &[u8], id: u32, data: &[u8]) -> NewResourceEdit {
    let type_id = crate::hash::sdbm(tag);
    let defaults = crate::create::type_defaults(type_id);
    NewResourceEdit {
        stream: stream.into(),
        header: crate::container::ustream::ResourceHeader {
            mem_policy: defaults.mem_policy,
            alignment: defaults.alignment,
            type_id,
            ids: [id; 4],
            data_size: 0,
        },
        data: data.to_vec(),
        storage: StorageChoice::Raw,
        before: None,
    }
}

/// Everything a mod does at once, in the own workspace of `project`:
/// replaced resources (a string, a texture, a material, the world grid),
/// resources added to a stream of the game (raw data at a place, a clone of
/// a texture), a new stream with a SimGroup, and a model imported into it as
/// a new set with its own collision and material.
fn fill(game: &GameInstance, mut ws: Workspace, dir: &Path) -> Workspace {
    let index = game.build_index(|_| {}).0;
    let none = |_: &Guid| false;

    // Replaced.
    let text = game.find_stream(mini::TEXT).unwrap();
    let mut table = StringTable::parse(&ws.current_data(game, text, 0).unwrap()).unwrap();
    table.strings[0][0] = "Si".into();
    ws.set_string_table(game, text, 0, &table).unwrap();
    let level = game.find_stream(mini::LEVEL).unwrap();
    let rgba: Vec<u8> = (0..16u8)
        .flat_map(|i| [i * 16, 255 - i * 16, 40, 255])
        .collect();
    let png = crate::resource::pixels::encode_png(4, 4, &rgba).unwrap();
    let pair = crate::resource::texture::TexturePair {
        header: mini::TEX_HEADER,
        data: mini::TEX_DATA,
    };
    ws.import_texture(game, level, pair, &png, true).unwrap();
    let mut material = ws.current_data(game, level, mini::MATERIAL).unwrap();
    let last = material.len() - 1;
    material[last] ^= 1;
    // Stored another way than the game's material is.
    let edit = crate::workspace::ResourceEdit {
        index: mini::MATERIAL,
        data: material,
        storage: StorageChoice::Refpack,
    };
    ws.set_resources(game, "Edit material", level, &[edit])
        .unwrap();

    // Added to a stream of the game: at a place, compressed; and a clone
    // of the texture (a pair of resources under new ids).
    let mut placed = raw(mini::LEVEL, b"vfx", 0x5101, &[7u8; 300]);
    placed.storage = StorageChoice::Refpack;
    let file = game.open_stream(level).unwrap();
    placed.before = Some(ResourceKey::new(
        mini::LEVEL,
        &file.layout().resources[mini::MESH].header,
    ));
    let last = raw(mini::LEVEL, b"vfx", 0x5102, b"at the end");
    ws.add_resources(game, "Add effects", &[placed, last])
        .unwrap();
    zone_ops::clone_resources(
        game,
        &index,
        &mut ws,
        &zone_ops::CloneParams {
            stream: mini::LEVEL.into(),
            indices: vec![mini::TEX_HEADER],
            ..Default::default()
        },
    )
    .unwrap();

    // A new stream with a SimGroup and its cell of the world grid.
    let stream = zone_ops::add_stream(
        game,
        &mut ws,
        &zone_ops::NewStreamParams {
            name: ZONE.into(),
            parent: mini::TOWN.into(),
            load: StreamLoad::OnDemand,
            ..Default::default()
        },
    )
    .unwrap();
    zone_ops::add_simgroup(
        game,
        &index,
        &mut ws,
        &zone_ops::NewGroupParams {
            stream: stream.name.clone(),
            copy_of: Some(format!("{}#{}", mini::YARD, mini::YARD_SIMGROUP)),
            ..Default::default()
        },
        &none,
    )
    .unwrap();
    zone_ops::add_zone_cell(
        game,
        &mut ws,
        &zone_ops::ZoneCellParams {
            level: mini::TOWN.into(),
            stream: stream.name.clone(),
            min: [30.0, 0.0, 30.0],
            max: [50.0, 10.0, 50.0],
            ..Default::default()
        },
    )
    .unwrap();

    // A model imported over a mesh of the game with a material of its own
    // name: the mesh is replaced, a clone of its material is added.
    let wall = dir.join("wall.glb");
    std::fs::write(&wall, mini::box_glb([3.0, 2.0, 0.5], "doctor_wall")).unwrap();
    let scene = crate::scene::read_file(&wall).unwrap();
    let mut assets = GameAssets::new(Arc::new(game.clone()), &index);
    let plan = zone_ops::plan_model_import(
        game,
        &index,
        &ws,
        &mut assets,
        &scene,
        &zone_ops::ImportModelParams {
            file: wall,
            template: format!("{}#{}", mini::LEVEL, mini::MESH),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(plan.summary.materials_new, 1, "{:?}", plan.summary);
    plan.commit(&mut ws, game).unwrap();

    // A model as a new set in the new stream, with collision of its own
    // and a material of its own name (a clone).
    let glb = dir.join("crate.glb");
    std::fs::write(&glb, mini::box_glb([2.0, 1.0, 3.0], "doctor_paint")).unwrap();
    let scene = crate::scene::read_file(&glb).unwrap();
    let mut assets = GameAssets::new(Arc::new(game.clone()), &index);
    let plan = zone_ops::plan_model_import(
        game,
        &index,
        &ws,
        &mut assets,
        &scene,
        &zone_ops::ImportModelParams {
            file: glb,
            template: format!("{PROPS}#{PROPS_MODEL}"),
            name: Some("doctor_crate".into()),
            stream: Some(stream.name.clone()),
            collision: Some("box".into()),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(
        plan.summary
            .added
            .iter()
            .any(|k| k.contains(":meshvolatile:")),
        "{:?}",
        plan.summary
    );
    plan.commit(&mut ws, game).unwrap();
    ws
}

/// The sources of `from` (and the data they are applied to) in a new
/// project folder: what version control holds — no workspace, no cache.
fn checkout(from: &Path, to: &Path) -> Project {
    fn copy(from: &Path, to: &Path) {
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
    Project::open(to).unwrap()
}

/// The changes of a workspace by key, and the streams it creates.
fn state(ws: &Workspace) -> (BTreeMap<String, Change>, Vec<NewStream>) {
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
}

/// Whether a key is of a kind a build encodes from a file (a picture, a
/// model): other data whenever the file changed.
fn encoded(key: &str) -> bool {
    [":tg4h:", ":tg4d:", ":mesh:", ":meshvolatile:"]
        .iter()
        .any(|t| key.contains(t))
}

#[test]
fn the_sources_of_a_workspace_build_the_same_workspace() {
    let dir = tempfile::tempdir().unwrap();
    let game = town(dir.path());
    let root = dir.path().join("mod");
    let mut project = document(&game, &root);
    let ws = fill(&game, project.workspace(&game).unwrap(), dir.path());
    let (changes, streams) = state(&ws);
    let added = changes.values().filter(|c| c.is_added()).count();
    assert!(added >= 9, "{:?}", changes.keys().collect::<Vec<_>>());
    assert_eq!(streams.len(), 1);

    // Sources for all of it.
    let report = project.export_workspace(&game, &ws, None).unwrap();
    assert!(report.notes.is_empty(), "{:?}", report.notes);
    assert_eq!(report.streams, [ZONE]);
    let covered: std::collections::BTreeSet<String> = project
        .entries()
        .flat_map(|(_, e, _)| e.keys.iter().map(ToString::to_string).collect::<Vec<_>>())
        .collect();
    assert_eq!(
        covered.iter().collect::<Vec<_>>(),
        changes.keys().collect::<Vec<_>>()
    );
    let kinds: Vec<(String, Kind, bool)> = project
        .entries()
        .map(|(id, e, _)| (id, e.kind, e.is_added()))
        .collect();
    let has = |part: &str, kind: Kind, added: bool| {
        kinds
            .iter()
            .any(|(id, k, a)| id.contains(part) && *k == kind && *a == added)
    };
    // The model of the new set, its metamodel, collision and material; the
    // SimGroup of the new stream; the clone of the texture; the raw data.
    assert!(has("doctor.str/mesh/", Kind::Model, true), "{kinds:?}");
    assert!(has("doctor.str/metamodel/", Kind::Logic, true), "{kinds:?}");
    assert!(has("doctor.str/hkx/", Kind::Raw, true), "{kinds:?}");
    assert!(has("doctor.str/material/", Kind::Raw, true), "{kinds:?}");
    assert!(has("doctor.str/simgroup/", Kind::Logic, true), "{kinds:?}");
    assert!(has("level.str/tg4h/", Kind::Texture, true), "{kinds:?}");
    assert!(has("level.str/tg4h/", Kind::Texture, false), "{kinds:?}");
    assert!(has("level.str/vfx/", Kind::Raw, true), "{kinds:?}");
    // The model imported over the game's: its mesh, and the material that
    // import cloned as a resource of its own.
    assert!(has("level.str/mesh/", Kind::Model, false), "{kinds:?}");
    assert!(has("level.str/material/", Kind::Raw, true), "{kinds:?}");
    assert!(has("town.str/wgi/", Kind::Raw, false), "{kinds:?}");
    // Written once: nothing is out of sync, nothing more to write.
    let status = project.status(&ws).unwrap();
    assert_eq!(status.to_build(), 0, "{status:?}");
    assert!(
        status.unknown_files.is_empty(),
        "{:?}",
        status.unknown_files
    );
    let again = project.export_workspace(&game, &ws, None).unwrap();
    assert_eq!(again, SourcesReport::default());
    // A manifest a person can read: the place and the stream are spelled out.
    let manifest = std::fs::read_to_string(root.join("src/gf2/town/doctor.str/stream.toml"));
    let manifest = manifest.unwrap();
    assert!(manifest.starts_with("format = 2\n"), "{manifest}");
    assert!(manifest.contains("[new]\nguid = \""), "{manifest}");
    assert!(manifest.contains("parent = \"gf2/town\"\nload = \"on_demand\""));
    assert!(manifest.contains("[[resource.added]]\nkey = \"gf2/town/doctor:"));
    let level = std::fs::read_to_string(root.join("src/gf2/level.str/stream.toml")).unwrap();
    assert!(
        level.contains("before = \"gf2/level:mesh:00003333\"")
            && level.contains("storage = \"refpack\""),
        "{level}"
    );
    // A folder of the game's resources only keeps the format it had.
    let text = std::fs::read_to_string(root.join("src/gf2/text/abcd.str/stream.toml")).unwrap();
    assert!(text.starts_with("format = 1\n"), "{text}");

    // A checkout of the sources builds the workspace again.
    let second = checkout(&root, &dir.path().join("second"));
    let mut ws2 = second.workspace(&game).unwrap();
    assert!(ws2.is_empty() && !ws2.is_saved());
    let status = second.status(&ws2).unwrap();
    assert_eq!(status.count(&EntryState::Modified), status.entries.len());
    assert!(status.new_streams.iter().all(|s| !s.built));
    let report = second
        .build(&game, &mut ws2, &BuildOptions::default())
        .unwrap();
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert_eq!(ws2.head(), 1, "one undoable operation");
    // Key for key and byte for byte, with the place, the order and the
    // storage of everything added: pictures and models too, because their
    // files are as they were written (the data they were written from is
    // kept with the sources).
    let (changes2, streams2) = state(&ws2);
    assert_eq!(streams2, streams);
    assert_eq!(
        changes2.keys().collect::<Vec<_>>(),
        changes.keys().collect::<Vec<_>>()
    );
    for (key, change) in &changes {
        assert_eq!(&changes2[key], change, "{key}");
    }
    // Nothing is left to build, and the build is the same on any computer.
    let status = second.status(&ws2).unwrap();
    assert_eq!(status.to_build(), 0, "{status:?}");
    let third = checkout(&dir.path().join("second"), &dir.path().join("third"));
    let mut ws3 = third.workspace(&game).unwrap();
    let report = third
        .build(&game, &mut ws3, &BuildOptions::default())
        .unwrap();
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert_eq!(state(&ws3), (changes2.clone(), streams2.clone()));

    // The second round: sources of the built workspace, built again, give
    // that workspace byte for byte — pictures and models too.
    checkout(&dir.path().join("second"), &dir.path().join("fresh"));
    std::fs::remove_dir_all(dir.path().join("fresh/src")).unwrap();
    let mut fresh = Project::open(dir.path().join("fresh")).unwrap();
    assert_eq!(fresh.entries().count(), 0);
    let report = fresh.export_workspace(&game, &ws2, None).unwrap();
    assert!(report.notes.is_empty(), "{:?}", report.notes);
    let fourth = checkout(&dir.path().join("fresh"), &dir.path().join("fourth"));
    let mut ws4 = fourth.workspace(&game).unwrap();
    let report = fourth
        .build(&game, &mut ws4, &BuildOptions::default())
        .unwrap();
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    let (changes4, streams4) = state(&ws4);
    assert_eq!(streams4, streams2);
    let differ: Vec<&String> = changes2
        .iter()
        .filter(|(key, change)| changes4.get(*key) != Some(change))
        .map(|(key, _)| key)
        .collect();
    assert!(differ.is_empty(), "{differ:?}");

    // A picture of an added texture is edited: it is encoded with the
    // format and the name of the texture as it was, under the same keys.
    let (texture_id, texture) = third
        .entries()
        .find(|(_, e, d)| e.kind == Kind::Texture && e.is_added() && d.contains("level"))
        .map(|(id, e, _)| (id, e.clone()))
        .unwrap();
    let rgba: Vec<u8> = (0..16u8).flat_map(|i| [20, i * 16, 200, 255]).collect();
    std::fs::write(
        third.root().join(&texture_id),
        crate::resource::pixels::encode_png(4, 4, &rgba).unwrap(),
    )
    .unwrap();
    let report = third
        .build(&game, &mut ws3, &BuildOptions::default())
        .unwrap();
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert_eq!(report.built.len(), 1, "{report:?}");
    let header = |ws: &Workspace| {
        crate::resource::tg4::Tg4Header::parse(&ws.data(&texture.keys[0]).unwrap().unwrap())
            .unwrap()
    };
    assert_eq!(header(&ws3).name, header(&ws2).name);
    assert_eq!(header(&ws3).format(), header(&ws2).format());
    assert_ne!(
        ws3.data(&texture.keys[1]).unwrap(),
        ws2.data(&texture.keys[1]).unwrap()
    );
    assert_eq!(
        ws3.change(&texture.keys[1]).unwrap().added,
        ws2.change(&texture.keys[1]).unwrap().added
    );
    assert!(encoded(&texture.keys[1].to_string()));

    // The model of the new set is edited: its file is imported again into
    // the same resources, under the ids they have, and nothing is added.
    let (model_id, model) = third
        .entries()
        .find(|(_, e, _)| e.kind == Kind::Model && e.is_added())
        .map(|(id, e, _)| (id, e.clone()))
        .unwrap();
    assert_eq!(model_id, "src/gf2/town/doctor.str/mesh/doctor_crate.glb");
    let parse = |ws: &Workspace| {
        let data = |k: &ResourceKey| ws.data(k).unwrap().unwrap();
        crate::resource::mesh::Model::parse(&data(&model.keys[0]), &data(&model.keys[1])).unwrap()
    };
    let before = parse(&ws3);
    assert_eq!(before.name, "doctor_crate");
    assert_eq!(before.aabb_max, [1.0, 1.0, 1.5]);
    std::fs::write(
        third.root().join(&model_id),
        mini::box_glb([4.0, 2.0, 6.0], mini::MATERIAL_NAME),
    )
    .unwrap();
    let report = third
        .build(&game, &mut ws3, &BuildOptions::default())
        .unwrap();
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert_eq!(report.built.len(), 1, "{report:?}");
    assert_eq!(report.built[0].id, model_id);
    assert!(
        !report.built[0].changed.is_empty()
            && report.built[0]
                .changed
                .iter()
                .all(|k| model.keys.contains(k)),
        "{report:?}"
    );
    let after = parse(&ws3);
    assert_eq!(after.name, "doctor_crate");
    assert_eq!(after.aabb_max, [2.0, 2.0, 3.0]);
    // The material of the set, by its name: the clone, not the game's.
    assert_eq!(
        after.records[0].material_hash,
        before.records[0].material_hash
    );
    let (changes3, _) = state(&ws3);
    assert_eq!(
        changes3.keys().collect::<Vec<_>>(),
        changes.keys().collect::<Vec<_>>()
    );
    assert_eq!(
        changes3[&model.keys[0].to_string()].added,
        changes2[&model.keys[0].to_string()].added
    );
}

/// The states of the entries that are not unchanged.
fn states(project: &Project, ws: &Workspace) -> Vec<(String, &'static str)> {
    project
        .status(ws)
        .unwrap()
        .entries
        .iter()
        .filter(|e| e.state != EntryState::Unchanged)
        .map(|e| (e.id.clone(), e.state.label()))
        .collect()
}

/// An entry of a resource the project adds, written by hand: built into an
/// added resource under the key it names, built again when its file
/// changes, and taken out when its file is removed.
#[test]
fn an_added_resource_written_by_hand_is_built_changed_and_removed() {
    let dir = tempfile::tempdir().unwrap();
    let game = town(dir.path());
    let root = dir.path().join("mod");
    document(&game, &root);
    let folder = root.join("src/gf2/level.str");
    std::fs::create_dir_all(folder.join("vfx")).unwrap();
    std::fs::write(folder.join("vfx/sparks.bin"), b"sparks").unwrap();
    std::fs::write(
        folder.join("stream.toml"),
        r#"format = 2
stream = "gf2/level"

[[resource]]
name = "sparks"
kind = "raw"
type = "vfx"
ids = "00005201"
policy = "default"
files = ["vfx/sparks.bin"]
keys = ["gf2/level:vfx:00005201"]

[[resource.added]]
key = "gf2/level:vfx:00005201"
before = "gf2/level:mesh:00003333"
"#,
    )
    .unwrap();
    let project = Project::open(&root).unwrap();
    let id = "src/gf2/level.str/vfx/sparks.bin";
    let key: ResourceKey = "gf2/level:vfx:00005201".parse().unwrap();
    let mut ws = project.workspace(&game).unwrap();
    assert_eq!(states(&project, &ws), [(id.to_owned(), "modified")]);
    let report = project
        .build(&game, &mut ws, &BuildOptions::default())
        .unwrap();
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert_eq!(report.built[0].changed, std::slice::from_ref(&key));
    let change = ws.change(&key).unwrap().clone();
    assert_eq!(ws.data(&key).unwrap().unwrap(), b"sparks");
    let added = change.added.unwrap();
    assert_eq!(
        (added.mem_policy, added.alignment, added.order),
        (crate::hash::sdbm(b"default"), 16, 0)
    );
    assert_eq!(added.before.unwrap().to_string(), "gf2/level:mesh:00003333");
    assert_eq!(states(&project, &ws), [(id.to_owned(), "built")]);

    // The file changes: the same resource with other data.
    std::fs::write(folder.join("vfx/sparks.bin"), b"more sparks").unwrap();
    assert_eq!(states(&project, &ws), [(id.to_owned(), "modified")]);
    project
        .build(&game, &mut ws, &BuildOptions::default())
        .unwrap();
    assert_eq!(ws.changes().count(), 1);
    assert_eq!(ws.data(&key).unwrap().unwrap(), b"more sparks");
    // Built again as it is: nothing is recorded.
    let forced = BuildOptions {
        force: true,
        ..Default::default()
    };
    let head = ws.head();
    let report = project.build(&game, &mut ws, &forced).unwrap();
    assert!(report.built[0].changed.is_empty() && ws.head() == head);
    // A place that is not a resource of the game's stream is an error of
    // the entry.
    let manifest = std::fs::read_to_string(folder.join("stream.toml")).unwrap();
    std::fs::write(
        folder.join("stream.toml"),
        manifest.replace("mesh:00003333", "mesh:0000dead"),
    )
    .unwrap();
    let broken = Project::open(&root).unwrap();
    let report = broken.build(&game, &mut ws, &forced).unwrap();
    assert!(
        report.errors[0].starts_with(id) && report.errors[0].contains("is not a resource of"),
        "{:?}",
        report.errors
    );
    std::fs::write(folder.join("stream.toml"), &manifest).unwrap();

    // The file is removed: the resource is no longer added, the entry is
    // out of the manifest. One operation, undone like any build.
    std::fs::remove_file(folder.join("vfx/sparks.bin")).unwrap();
    assert_eq!(states(&project, &ws), [(id.to_owned(), "removed")]);
    let dry = BuildOptions {
        dry_run: true,
        ..Default::default()
    };
    let report = project.build(&game, &mut ws, &dry).unwrap();
    assert_eq!(report.built[0].notes, ["removed"]);
    assert!(ws.change(&key).is_some());
    let report = project
        .build(&game, &mut ws, &BuildOptions::default())
        .unwrap();
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert_eq!(report.built[0].changed, std::slice::from_ref(&key));
    assert!(ws.is_empty());
    let project = Project::open(&root).unwrap();
    assert_eq!(project.entries().count(), 0);
    assert_eq!(project.status(&ws).unwrap().to_build(), 0);
    ws.undo().unwrap();
    assert_eq!(ws.data(&key).unwrap().unwrap(), b"more sparks");
}

/// The folder of a stream the game does not have, written by hand: a build
/// creates the stream with its resources, follows a change of its
/// description, and removes it when the folder is gone.
#[test]
fn a_stream_written_by_hand_is_created_changed_and_removed() {
    let dir = tempfile::tempdir().unwrap();
    let game = town(dir.path());
    let root = dir.path().join("mod");
    document(&game, &root);
    let folder = root.join("src/gf2/town/annex.str");
    std::fs::create_dir_all(folder.join("vfx")).unwrap();
    std::fs::write(folder.join("vfx/smoke.bin"), b"smoke").unwrap();
    let manifest = r#"format = 2
stream = "gf2/town/annex"

[new]
guid = "0102030405060708090a0b0c0d0e0f10"
parent = "gf2/town"
tocs = ["gf2/level.toc"]

[[resource]]
name = "smoke"
kind = "raw"
type = "vfx"
ids = "00005301"
policy = "default"
files = ["vfx/smoke.bin"]
keys = ["gf2/town/annex:vfx:00005301"]

[[resource.added]]
key = "gf2/town/annex:vfx:00005301"
"#;
    std::fs::write(folder.join("stream.toml"), manifest).unwrap();
    let project = Project::open(&root).unwrap();
    let mut ws = project.workspace(&game).unwrap();
    let status = project.status(&ws).unwrap();
    assert_eq!(status.new_streams.len(), 1);
    assert!(!status.new_streams[0].built);
    assert_eq!(status.to_build(), 2, "the stream and its resource");
    let dry = BuildOptions {
        dry_run: true,
        ..Default::default()
    };
    let report = project.build(&game, &mut ws, &dry).unwrap();
    assert_eq!(report.built[0].id, "src/gf2/town/annex.str/stream.toml");
    assert!(!ws.is_saved());
    // Written by hand and not built: the sources of the workspace, which
    // does not create the stream yet, leave the folder alone.
    let report = Project::open(&root)
        .unwrap()
        .export_workspace(&game, &ws, None)
        .unwrap();
    assert_eq!(report, SourcesReport::default());
    assert!(folder.join("stream.toml").is_file() && folder.join("vfx/smoke.bin").is_file());

    let report = project
        .build(&game, &mut ws, &BuildOptions::default())
        .unwrap();
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert_eq!(ws.head(), 1, "one undoable operation");
    let stream = ws.new_stream("gf2/town/annex").unwrap().clone();
    let guid: [u8; 16] = std::array::from_fn(|i| i as u8 + 1);
    assert_eq!(stream.hash, crate::toc::guid_hash(&guid));
    assert_eq!(
        (stream.parent.as_str(), stream.with_parent, stream.auto_load),
        ("gf2/town", false, false)
    );
    assert_eq!(ws.new_stream_resources("gf2/town/annex").len(), 1);
    assert_eq!(project.status(&ws).unwrap().to_build(), 0);
    // The game as the workspace will leave it has the stream.
    let staged = ws.stage(&game).unwrap();
    assert!(staged.find_stream("gf2/town/annex").is_some());

    // How it is loaded changes in the file: the stream follows, its
    // resource stays.
    let loaded = manifest.replace(
        "parent = \"gf2/town\"",
        "parent = \"gf2/town\"\nload = \"auto_load\"",
    );
    std::fs::write(folder.join("stream.toml"), loaded).unwrap();
    let mut project = Project::open(&root).unwrap();
    assert_eq!(project.status(&ws).unwrap().to_build(), 1);
    // Not built yet: the description is the author's, not overwritten by
    // the one the workspace still has.
    let report = project.export_workspace(&game, &ws, None).unwrap();
    assert!(
        report.notes.len() == 1 && report.notes[0].contains("is not built"),
        "{report:?}"
    );
    assert!(
        std::fs::read_to_string(folder.join("stream.toml"))
            .unwrap()
            .contains("load = \"auto_load\"")
    );
    project
        .build(&game, &mut ws, &BuildOptions::default())
        .unwrap();
    let stream = ws.new_stream("gf2/town/annex").unwrap();
    assert!(stream.with_parent && stream.auto_load);
    assert_eq!(ws.new_stream_resources("gf2/town/annex").len(), 1);
    assert_eq!(project.status(&ws).unwrap().to_build(), 0);

    // A parent the game does not have is an error of the folder.
    std::fs::write(
        folder.join("stream.toml"),
        manifest.replace("parent = \"gf2/town\"", "parent = \"gf2/nowhere\""),
    )
    .unwrap();
    let report = Project::open(&root)
        .unwrap()
        .build(&game, &mut ws, &BuildOptions::default())
        .unwrap();
    assert!(
        report.errors[0].starts_with("src/gf2/town/annex.str/stream.toml"),
        "{:?}",
        report.errors
    );

    // The folder is removed: the stream goes with what was added to it.
    std::fs::remove_dir_all(&folder).unwrap();
    let project = Project::open(&root).unwrap();
    let status = project.status(&ws).unwrap();
    assert_eq!(
        status.removed_streams,
        [(
            "src/gf2/town/annex.str".to_owned(),
            "gf2/town/annex".to_owned()
        )]
    );
    let report = project
        .build(&game, &mut ws, &BuildOptions::default())
        .unwrap();
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert!(ws.is_empty());
    assert_eq!(project.status(&ws).unwrap().to_build(), 0);
}

/// A project made of the changes of a workspace holds what the workspace
/// adds and creates; a project made of streams takes what is added to them.
#[test]
fn a_project_from_a_workspace_keeps_what_it_adds() {
    let dir = tempfile::tempdir().unwrap();
    let game = town(dir.path());
    let mut ws = fill(&game, Workspace::for_game(&game, None).unwrap(), dir.path());
    let before = state(&ws);
    let new = NewProject {
        from_workspace: true,
        ..Default::default()
    };
    let (project, report) =
        create_from(&game, &ws, None, &dir.path().join("whole"), &new, None).unwrap();
    assert_eq!(report.new_streams, 1);
    assert!(report.notes.is_empty(), "{:?}", report.notes);
    let keys: std::collections::BTreeSet<String> = project
        .entries()
        .flat_map(|(_, e, _)| e.keys.iter().map(ToString::to_string).collect::<Vec<_>>())
        .collect();
    assert_eq!(
        keys.iter().collect::<Vec<_>>(),
        before.0.keys().collect::<Vec<_>>()
    );
    assert_eq!(project.status(&ws).unwrap().to_build(), 0);
    // Everything is taken out of the workspace: a build puts it back as it
    // was, the stream and the places of the added resources too.
    ws.revert_all().unwrap();
    assert!(ws.is_empty());
    let status = project.status(&ws).unwrap();
    assert_eq!(status.count(&EntryState::OutOfSync), status.entries.len());
    let report = project
        .build(&game, &mut ws, &BuildOptions::default())
        .unwrap();
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    let after = state(&ws);
    assert_eq!(after.1, before.1);
    assert_eq!(
        after.0.keys().collect::<Vec<_>>(),
        before.0.keys().collect::<Vec<_>>()
    );
    assert_eq!(after.0, before.0);
    // The mod of the project has the stream and the added resources.
    let out = dir.path().join("whole.zip");
    let built = project.build_mod(&ws, &out, None).unwrap();
    assert_eq!(built.new_streams, 1);
    assert!(built.added >= 9, "{built:?}");

    // Of one stream: its resources and what the workspace adds to it; not
    // the stream the workspace creates.
    let new = NewProject {
        streams: vec![mini::LEVEL.into()],
        ..Default::default()
    };
    let (project, report) =
        create_from(&game, &ws, None, &dir.path().join("level"), &new, None).unwrap();
    assert_eq!(report.new_streams, 0);
    let added: Vec<String> = project
        .entries()
        .filter(|(_, e, _)| e.is_added())
        .map(|(id, _, _)| id)
        .collect();
    assert_eq!(added.len(), 4, "{added:?}");
    assert!(added.iter().all(|id| id.starts_with("src/gf2/level.str/")));
    assert_eq!(project.status(&ws).unwrap().to_build(), 0);
}

/// The sources follow the workspace: an added resource edited there is
/// pulled into its file, one removed there leaves the sources, and so does
/// a stream the workspace no longer creates.
#[test]
fn the_sources_follow_what_the_workspace_adds_and_removes() {
    let dir = tempfile::tempdir().unwrap();
    let game = town(dir.path());
    let root = dir.path().join("mod");
    let mut project = document(&game, &root);
    let mut ws = project.workspace(&game).unwrap();
    // Nothing yet: nothing is written.
    assert_eq!(
        project.export_workspace(&game, &ws, None).unwrap(),
        SourcesReport::default()
    );
    let sparks = raw(mini::LEVEL, b"vfx", 0x5401, b"sparks");
    let key = ResourceKey {
        stream: mini::LEVEL.into(),
        type_id: sparks.header.type_id,
        ids: sparks.header.ids,
    };
    ws.add_resources(&game, "Add sparks", &[sparks]).unwrap();
    let stream = zone_ops::add_stream(
        &game,
        &mut ws,
        &zone_ops::NewStreamParams {
            name: ZONE.into(),
            parent: mini::TOWN.into(),
            load: StreamLoad::WithParent,
            ..Default::default()
        },
    )
    .unwrap();
    let smoke = raw(ZONE, b"vfx", 0x5402, b"smoke");
    ws.add_resources(&game, "Add smoke", &[smoke]).unwrap();
    let report = project.export_workspace(&game, &ws, None).unwrap();
    assert_eq!(report.added.len(), 2, "{report:?}");
    assert_eq!(report.streams, [ZONE]);
    let id = "src/gf2/level.str/vfx/00005401.bin";
    assert_eq!(report.added[0], id);
    let manifest = root.join("src/gf2/town/doctor.str/stream.toml");
    let described = |load: &str| {
        std::fs::read_to_string(&manifest)
            .unwrap()
            .contains(&format!("load = \"{load}\""))
    };
    assert!(described("with_parent"));

    // Edited in the workspace: out of sync; a pull takes it into the file.
    let edited = (key.clone(), Some(b"brighter sparks".to_vec()));
    ws.change_resources_and_added(&game, "Edit sparks", &[], &[], &[edited])
        .unwrap();
    assert_eq!(states(&project, &ws), [(id.to_owned(), "out of sync")]);
    assert_eq!(project.pull(&game, &ws, &[]).unwrap(), [id]);
    assert_eq!(std::fs::read(root.join(id)).unwrap(), b"brighter sparks");
    assert!(states(&project, &ws).is_empty());

    // Another one added, the stream loaded another way: written by the
    // next export, which leaves the rest alone.
    let more = raw(mini::LEVEL, b"vfx", 0x5403, b"embers");
    ws.add_resources(&game, "Add embers", &[more]).unwrap();
    let mut changed = stream.clone();
    changed.auto_load = true;
    ws.put_stream(&game, changed).unwrap();
    let report = project.export_workspace(&game, &ws, None).unwrap();
    assert_eq!(report.added, ["src/gf2/level.str/vfx/00005403.bin"]);
    assert_eq!(report.streams, [ZONE]);
    assert!(report.pulled.is_empty() && report.removed.is_empty());
    assert!(described("auto_load"));
    assert_eq!(project.status(&ws).unwrap().to_build(), 0);

    // Removed in the workspace: the entry and its file go, by a pull...
    ws.change_resources_and_added(&game, "Remove sparks", &[], &[], &[(key.clone(), None)])
        .unwrap();
    assert_eq!(states(&project, &ws), [(id.to_owned(), "out of sync")]);
    assert_eq!(project.pull(&game, &ws, &[]).unwrap(), [id]);
    assert!(!root.join(id).exists());
    assert!(project.entry(id).is_none());
    assert!(Project::open(&root).unwrap().entry(id).is_none());
    // ... and a stream that is no longer created, by an export.
    ws.remove_stream(ZONE).unwrap();
    let report = project.export_workspace(&game, &ws, None).unwrap();
    assert_eq!(
        report.removed,
        [
            "src/gf2/town/doctor.str/vfx/00005402.bin",
            "src/gf2/town/doctor.str"
        ]
    );
    assert!(!manifest.exists());
    let project = Project::open(&root).unwrap();
    assert_eq!(project.entries().count(), 1);
    assert_eq!(project.status(&ws).unwrap().to_build(), 0);

    // A project that builds into the workspace of the game does not take
    // all of that workspace.
    let new = NewProject {
        streams: vec![mini::TEXT.into()],
        ..Default::default()
    };
    let game_ws = Workspace::for_game(&game, None).unwrap();
    let to = dir.path().join("streams");
    let (mut of_streams, _) = create_from(&game, &game_ws, None, &to, &new, None).unwrap();
    let error = of_streams
        .export_workspace(&game, &game_ws, None)
        .unwrap_err()
        .to_string();
    assert!(error.contains("its own workspace"), "{error}");
}

/// What cannot be written as its kind stays raw data with its ids and its
/// place: nothing the workspace adds is lost. Here a new model set whose
/// template keeps the volatile data of its mesh apart from it (the yard of
/// the miniature game), so the clone has none and is not a readable model.
#[test]
fn what_cannot_be_converted_stays_raw_with_its_ids_and_place() {
    let dir = tempfile::tempdir().unwrap();
    let game = town(dir.path());
    let root = dir.path().join("mod");
    let mut project = document(&game, &root);
    let mut ws = project.workspace(&game).unwrap();
    let index = game.build_index(|_| {}).0;
    let stream = zone_ops::add_stream(
        &game,
        &mut ws,
        &zone_ops::NewStreamParams {
            name: ZONE.into(),
            parent: mini::TOWN.into(),
            ..Default::default()
        },
    )
    .unwrap();
    let glb = dir.path().join("crate.glb");
    std::fs::write(&glb, mini::box_glb([2.0, 1.0, 3.0], mini::MATERIAL_NAME)).unwrap();
    let scene = crate::scene::read_file(&glb).unwrap();
    let mut assets = GameAssets::new(Arc::new(game.clone()), &index);
    let plan = zone_ops::plan_model_import(
        &game,
        &index,
        &ws,
        &mut assets,
        &scene,
        &zone_ops::ImportModelParams {
            file: glb,
            template: format!("{}#{}", mini::YARD, mini::YARD_MODEL),
            name: Some("doctor_crate".into()),
            stream: Some(stream.name.clone()),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(
        plan.summary
            .warnings
            .iter()
            .any(|w| w.contains("no meshvolatile")),
        "{:?}",
        plan.summary.warnings
    );
    plan.commit(&mut ws, &game).unwrap();

    let report = project.export_workspace(&game, &ws, None).unwrap();
    assert_eq!(report.notes.len(), 1, "{:?}", report.notes);
    assert!(
        report.notes[0].contains(":mesh:") && report.notes[0].contains("kept as raw data"),
        "{:?}",
        report.notes
    );
    let (id, mesh) = project
        .entries()
        .find(|(_, e, _)| e.type_tag == "mesh")
        .map(|(id, e, _)| (id, e.clone()))
        .unwrap();
    assert!(id.ends_with(".bin"), "{id}");
    assert_eq!(mesh.kind, Kind::Raw);
    assert!(mesh.is_added() && mesh.template.is_empty());
    assert_eq!(project.status(&ws).unwrap().to_build(), 0);

    let second = checkout(&root, &dir.path().join("second"));
    let mut ws2 = second.workspace(&game).unwrap();
    let report = second
        .build(&game, &mut ws2, &BuildOptions::default())
        .unwrap();
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert_eq!(state(&ws2), state(&ws));
}

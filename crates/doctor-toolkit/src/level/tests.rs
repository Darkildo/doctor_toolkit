use std::collections::HashMap;
use std::sync::Arc;

use super::model::{self, MetaModels};
use super::*;
use crate::resource::metamodel::{
    Condition, Part, PropValue, Property, ResourceRef, State, Variable,
};
use crate::resource::simgroup::Hash32;

/// Metamodels by id for [`model::expand`].
#[derive(Default)]
struct Fake(HashMap<u32, Arc<MetaModel>>);

impl MetaModels for Fake {
    fn metamodel(&mut self, id: u32) -> Option<Arc<MetaModel>> {
        self.0.get(&id).cloned()
    }
}

fn empty(id: u32) -> MetaModel {
    MetaModel {
        version: 14,
        ids: [id; 4],
        header: [0; 5],
        resources: Vec::new(),
        states: Vec::new(),
        variables: Vec::new(),
        properties: Vec::new(),
        conditions: Vec::new(),
        parts: Vec::new(),
        sub_objects: Vec::new(),
    }
}

fn resource(type_id: u32, id: u32) -> ResourceRef {
    ResourceRef {
        hash: Hash32(0),
        id: Hash32(id),
        type_id: Hash32(type_id),
        type_name: None,
        ids: [Hash32(id); 4],
    }
}

fn property(class: &str, name: &str, value: PropValue) -> Property {
    Property {
        name: name.into(),
        hash: Hash32(0),
        class: class.into(),
        index: 1,
        type_flag: 0,
        value,
    }
}

fn translation(x: f32, y: f32, z: f32) -> RowMatrix {
    let mut m = IDENTITY;
    m[3] = [x, y, z, 1.0];
    m
}

impl Fake {
    /// Adds a part with `props` to state `state` (created when missing).
    fn part(mm: &mut MetaModel, state: (&str, &str), kind: &str, props: Vec<Property>) {
        let s = match mm.states.iter().position(|s| s.name == state.1) {
            Some(s) => s,
            None => {
                mm.states.push(State {
                    type_hash: 0,
                    type_name: state.0.into(),
                    name: state.1.into(),
                    guid: Guid::default(),
                    flags: 1,
                    properties: Vec::new(),
                    parts: Vec::new(),
                    conditions: Vec::new(),
                });
                mm.states.len() - 1
            }
        };
        let first = mm.properties.len();
        mm.properties.extend(props);
        mm.parts.push(Part {
            type_hash: 0,
            type_name: kind.into(),
            name: kind.into(),
            guid: Guid::default(),
            flags: 2,
            properties: (first..mm.properties.len()).collect(),
        });
        mm.states[s].parts.push(mm.parts.len() - 1);
    }
}

#[test]
fn models_expand_into_meshes_nested_models_lods_and_trees() {
    let mut fake = Fake::default();
    // A nested destructible: one mesh.
    let mut window = empty(2);
    window.resources.push(resource(TYPE_MESH, 0x20));
    Fake::part(
        &mut window,
        ("DestructibleState", "DestructibleState_0"),
        "ModelPart",
        vec![property("ModelPart", "Model", PropValue::Resource(0))],
    );
    // The broken state is not drawn.
    window.resources.push(resource(TYPE_MESH, 0x21));
    Fake::part(
        &mut window,
        ("DestructibleState", "DestructibleState_1"),
        "ModelPart",
        vec![property("ModelPart", "Model", PropValue::Resource(1))],
    );
    fake.0.insert(2, Arc::new(window));

    let mut building = empty(1);
    building.resources = vec![
        resource(TYPE_MESH, 0x10),
        resource(TYPE_METAMODEL, 2),
        resource(TYPE_MESH, 0x11),
        resource(TYPE_MESH, 0x12),
        resource(TYPE_TREE, 0x30),
        resource(TYPE_MESH, 0x13),
        resource(TYPE_METAMODEL, 99),
    ];
    Fake::part(
        &mut building,
        ("State", "State"),
        "ModelPart",
        vec![property("ModelPart", "Model", PropValue::Resource(0))],
    );
    Fake::part(
        &mut building,
        ("State", "State"),
        "Destructible",
        vec![
            property("Entity", "CMD_SetModel", PropValue::Resource(1)),
            property(
                "CSystemCommands",
                "CMD_LoadMatrix",
                PropValue::Matrix(translation(5.0, 0.0, 0.0)),
            ),
        ],
    );
    Fake::part(
        &mut building,
        ("State", "State"),
        "TreeModelPart",
        vec![
            property("TreeModelPart", "TreeModel", PropValue::Resource(4)),
            property(
                "TreeModelPart",
                "CMD_LoadMatrix",
                PropValue::Matrix(translation(0.0, 0.0, 7.0)),
            ),
        ],
    );
    Fake::part(
        &mut building,
        ("State", "State"),
        "MinimapPart",
        vec![property("MinimapPart", "Model", PropValue::Resource(5))],
    );
    Fake::part(
        &mut building,
        ("State", "State"),
        "MetaModelPart",
        vec![property(
            "MetaModelPart",
            "fileName",
            PropValue::Resource(6),
        )],
    );
    // Two LODs on the float variable `distance`.
    building.variables.push(Variable {
        name: "distance".into(),
        hash: Hash32(0),
        kind: 4,
    });
    for (name, mesh, near, far) in [("near", 2, 0.0f32, 25.0f32), ("far", 3, 25.0, 1e4)] {
        Fake::part(
            &mut building,
            ("LodState", name),
            "ModelPart",
            vec![property("ModelPart", "Model", PropValue::Resource(mesh))],
        );
        let s = building.states.len() - 1;
        for (operator, value) in [(2, near), (3, far)] {
            building.conditions.push(Condition {
                value: value.to_bits(),
                variable: 0,
                operator,
            });
            building.states[s]
                .conditions
                .push(building.conditions.len() - 1);
        }
    }
    // A transition is not a state to draw.
    Fake::part(
        &mut building,
        ("Transition", "Transition"),
        "ModelPart",
        vec![property("ModelPart", "Model", PropValue::Resource(0))],
    );
    fake.0.insert(1, Arc::new(building));

    let desc = model::expand(1, &mut fake);
    let meshes: Vec<(u32, [f32; 4], Option<LodRange>)> = desc
        .pieces
        .iter()
        .map(|p| (p.mesh, p.matrix[3], p.lod))
        .collect();
    assert_eq!(
        meshes,
        vec![
            (0x10, [0.0, 0.0, 0.0, 1.0], None),
            (0x20, [5.0, 0.0, 0.0, 1.0], None),
            (0x11, [0.0, 0.0, 0.0, 1.0], Some([0.0, 25.0])),
            (0x12, [0.0, 0.0, 0.0, 1.0], Some([25.0, 1e4])),
        ]
    );
    assert_eq!(desc.trees.len(), 1);
    assert_eq!(desc.trees[0].matrix[3], [0.0, 0.0, 7.0, 1.0]);
    assert_eq!(desc.missing, vec![99]);
    assert_eq!(desc.metamodels, 2);
    // A missing model and a cycle.
    assert_eq!(model::expand(5, &mut fake).missing, vec![5]);
    let mut cyclic = empty(7);
    cyclic.resources.push(resource(TYPE_METAMODEL, 7));
    Fake::part(
        &mut cyclic,
        ("State", "State"),
        "MetaModelPart",
        vec![property(
            "MetaModelPart",
            "fileName",
            PropValue::Resource(0),
        )],
    );
    fake.0.insert(7, Arc::new(cyclic));
    assert_eq!(model::expand(7, &mut fake).metamodels, 1);
}

#[test]
fn nested_models_resolve_by_guid_not_by_path_hash() {
    // As in the game: the path hash names the unsplit original (20), the
    // GUID the split metamodel of the same level of detail (21).
    let mut fake = Fake::default();
    let mut nested_ref = resource(TYPE_METAMODEL, 21);
    nested_ref.id = Hash32(20);
    let mut outer = empty(1);
    outer.resources.push(nested_ref);
    Fake::part(
        &mut outer,
        ("State", "State"),
        "MetaModelPart",
        vec![property(
            "MetaModelPart",
            "fileName",
            PropValue::Resource(0),
        )],
    );
    let with_mesh = |id: u32, mesh: u32| {
        let mut mm = empty(id);
        mm.resources.push(resource(TYPE_MESH, mesh));
        Fake::part(
            &mut mm,
            ("State", "State"),
            "ModelPart",
            vec![property("ModelPart", "Model", PropValue::Resource(0))],
        );
        Arc::new(mm)
    };
    fake.0.insert(1, Arc::new(outer));
    fake.0.insert(20, with_mesh(20, 0x200));
    fake.0.insert(21, with_mesh(21, 0x210));
    let desc = model::expand(1, &mut fake);
    assert_eq!(desc.pieces.len(), 1);
    assert_eq!(desc.pieces[0].mesh, 0x210);
    assert!(desc.missing.is_empty());
}

#[test]
fn levels_load_groups_and_honour_overrides() {
    let (_dir, game) = crate::workspace::tests::fake_game();
    let game = Arc::new(game);
    let index = game.build_index(|_| {}).0;
    let lookup = Arc::new(Lookup::new(&index));
    let level_stream = game.find_stream("gf2/level").unwrap();
    let catalog = Catalog::new(&game, &index);
    assert_eq!(catalog.zones.len(), 1);
    assert_eq!(catalog.zones[0].stream, level_stream);
    let scope = Scope::new(&game, &game.stream_parents().unwrap(), &[level_stream]);
    assert_eq!(scope.streams, vec![level_stream]);
    let source = Arc::new(LevelSource::new(
        game.clone(),
        lookup.clone(),
        Overrides::default(),
        scope.clone(),
    ));
    let level = Level::load(source, &[level_stream]).unwrap();
    assert_eq!(level.groups.len(), 1);
    let group = &level.groups[0];
    assert_eq!(group.objects.len(), 1);
    assert_eq!(group.objects[0].class_name, Some("Entity"));
    assert_eq!(group.objects[0].transform, None);
    let coverage = level.coverage();
    assert_eq!((coverage.objects, coverage.simgroups), (1, 1));

    // The group as the workspace has it.
    let mut edited = group.group.clone();
    let guid = crate::resource::simgroup_edit::new_object_guid("t", |_| false);
    edited.duplicate_object(0, guid).unwrap();
    let mut overrides = Overrides::default();
    overrides
        .groups
        .insert(group.location, Arc::new(edited.encode()));
    let source = Arc::new(LevelSource::new(game, lookup, overrides, scope));
    let mut level = Level::load(source, &[level_stream]).unwrap();
    assert_eq!(level.groups[0].objects.len(), 2);
    assert!(level.guids().contains(&guid));
    level.set_group(0, &edited.encode()).unwrap();
    assert_eq!(level.groups[0].objects[1].guid, guid);
    assert!(level.set_group(3, &[]).is_err());
}

#[test]
fn scope_prefers_near_streams() {
    let scope = Scope {
        streams: vec![4, 2],
        rank: HashMap::from([(4, 0), (2, 1)]),
    };
    let at = |stream| Location {
        stream,
        resource: 0,
    };
    assert_eq!(scope.pick(&[at(9), at(2), at(4)]), Some((at(4), true)));
    assert_eq!(scope.pick(&[at(9), at(8)]), Some((at(9), false)));
    assert_eq!(scope.pick(&[]), None);
}

/// A game with one zone: a metamodel (id 0x5151, showing mesh 0xABCD that
/// is not in the game) and a SimGroup with one object.
fn zone_game() -> (tempfile::TempDir, GameInstance) {
    use crate::container::ustream::{FileHeader, StreamLayout, StreamWriter};
    use crate::workspace::tests::{header, toc_bytes};
    let mut zone = StreamWriter::new(&FileHeader::GF2_PC, crate::io::Endian::Little);
    let mut mm = crate::resource::metamodel::tests::sample();
    for k in 0..4 {
        mm[0x10 + 4 * k..0x14 + 4 * k].copy_from_slice(&0x5151u32.to_le_bytes());
    }
    zone.push_resource(&header(b"metamodel", [0x5151; 4]), &mm, Storage::RefPack)
        .unwrap();
    zone.push_resource(
        &header(b"simgroup", [0; 4]),
        &crate::resource::simgroup::tests::sample(),
        Storage::RefPack,
    )
    .unwrap();
    let zone = zone.finish();
    let size = StreamLayout::parse(&zone).unwrap().total_data_size();
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir_all(root.join("gf2")).unwrap();
    std::fs::write(root.join("godfather2.exe"), b"MZ fake").unwrap();
    std::fs::write(root.join("gf2/zone.str"), &zone).unwrap();
    let table = crate::toc::Toc::parse(&toc_bytes(&[("gf2/zone", size)])).unwrap();
    std::fs::write(root.join("gf2/zone.toc"), table.to_bytes().unwrap()).unwrap();
    let game = GameInstance::open(root).unwrap();
    (dir, game)
}

use crate::container::ustream::Storage;

#[test]
fn objects_are_duplicated_removed_and_added_in_the_workspace() {
    use super::edit::{self, ModelChoice};
    let (dir, game) = zone_game();
    let index = game.build_index(|_| {}).0;
    let zone = game.find_stream("gf2/zone").unwrap();
    let group = Location {
        stream: zone,
        resource: 1,
    };
    let mm_at = Location {
        stream: zone,
        resource: 0,
    };
    let mut ws =
        crate::workspace::Workspace::for_game(&game, Some(&dir.path().join("ws"))).unwrap();
    let none = |_: &Guid| false;

    // No CSystemCommands packet: no transform to set.
    assert!(edit::set_transform(&game, &mut ws, group, 0, IDENTITY).is_err());
    let (copy, guid) = edit::duplicate(&game, &mut ws, group, 0, [1.0, 0.0, 0.0], &none).unwrap();
    assert_eq!(copy, 1);
    let g = edit::current_group(&game, &ws, group).unwrap();
    assert_eq!(g.objects.len(), 2);
    assert_eq!(g.objects[1].guid, guid);

    let added = edit::add(
        &game,
        &index,
        &mut ws,
        group,
        (group, 0),
        ModelChoice::Existing(0x5151),
        None,
        &none,
        true,
    )
    .unwrap();
    assert_eq!((added.object, added.model), (2, Some(0x5151)));
    assert!(added.cloned.is_empty());

    let cloned = edit::add(
        &game,
        &index,
        &mut ws,
        group,
        (group, 0),
        ModelChoice::Clone(mm_at),
        None,
        &none,
        true,
    )
    .unwrap();
    assert_eq!(cloned.cloned.len(), 1, "the mesh is not in the stream");
    let new_id = cloned.model.unwrap();
    assert_ne!(new_id, 0x5151);
    assert_eq!(cloned.cloned[0].ids, [new_id; 4]);
    let g = edit::current_group(&game, &ws, group).unwrap();
    assert_eq!(g.objects.len(), 4);
    assert_eq!(g.model(&g.objects[3]), Some(new_id));

    // Own collision needs a collision part in the metamodel; nothing is recorded.
    let operations = ws.operations().len();
    let cage = crate::resource::hkx::collision::CollisionMesh {
        positions: vec![[0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
        triangles: vec![[0, 1, 2], [0, 2, 3], [0, 3, 1], [1, 3, 2]],
    };
    let error = edit::add(
        &game,
        &index,
        &mut ws,
        group,
        (group, 0),
        ModelChoice::CloneWithCollision(
            mm_at,
            crate::resource::hkx::collision::CollisionKind::Mesh,
            Some(Arc::new(cage)),
        ),
        None,
        &none,
        true,
    )
    .unwrap_err();
    assert!(error.to_string().contains("collision part"), "{error}");
    assert_eq!(ws.operations().len(), operations);

    // The level shows the clone through the overrides of the workspace.
    let data = ws.data(&cloned.cloned[0]).unwrap().unwrap();
    let mut overrides = Overrides::default();
    overrides
        .by_id
        .insert((TYPE_METAMODEL, new_id), (zone, Arc::new(data)));
    overrides
        .groups
        .insert(group, Arc::new(ws.current_data(&game, zone, 1).unwrap()));
    let game = Arc::new(game);
    let lookup = Arc::new(Lookup::new(&index));
    let scope = Scope::new(&game, &game.stream_parents().unwrap(), &[zone]);
    let source = Arc::new(LevelSource::new(game.clone(), lookup, overrides, scope));
    let level = Level::load(source, &[zone]).unwrap();
    let model = &level.models[&new_id];
    assert!(model.found.is_some_and(|f| f.in_scope()));
    assert_eq!(model.desc.pieces.len(), 1);
    assert_eq!(model.desc.pieces[0].mesh, 0xABCD);
    let coverage = level.coverage();
    assert_eq!(coverage.with_model, 2);
    assert_eq!(coverage.meshes_missing, 1);

    // Removal; undo brings the object back.
    let dangling = edit::remove(&game, &mut ws, group, 1).unwrap();
    assert_eq!(dangling, 0);
    assert_eq!(
        edit::current_group(&game, &ws, group)
            .unwrap()
            .objects
            .len(),
        3
    );
    ws.undo().unwrap();
    assert_eq!(
        edit::current_group(&game, &ws, group)
            .unwrap()
            .objects
            .len(),
        4
    );
    assert!(edit::remove(&game, &mut ws, group, 9).is_err());
}

#[test]
fn models_are_cloned_into_streams_holding_applied_additions() {
    use super::edit::{self, ModelChoice};
    let (dir, game) = zone_game();
    let index = game.build_index(|_| {}).0;
    let zone = game.find_stream("gf2/zone").unwrap();
    let mm_at = Location {
        stream: zone,
        resource: 0,
    };
    let mut ws =
        crate::workspace::Workspace::for_game(&game, Some(&dir.path().join("ws"))).unwrap();
    let none = |_: &Guid| false;
    let group = Location {
        stream: zone,
        resource: 1,
    };
    let first = edit::add(
        &game,
        &index,
        &mut ws,
        group,
        (group, 0),
        ModelChoice::Clone(mm_at),
        None,
        &none,
        true,
    )
    .unwrap();
    ws.apply(&game, Default::default()).unwrap();
    // The game file now holds the clone right after its source, and the
    // SimGroup moved to index 2.
    let layout = game.open_stream(zone).unwrap().layout().clone();
    assert_eq!(layout.resources.len(), 3);
    assert_eq!(layout.resources[1].header.ids, first.cloned[0].ids);
    let group = Location {
        stream: zone,
        resource: 2,
    };
    let second = edit::add(
        &game,
        &index,
        &mut ws,
        group,
        (group, 0),
        ModelChoice::Clone(mm_at),
        None,
        &none,
        true,
    )
    .unwrap();
    assert_ne!(second.model, first.model);
    ws.apply(&game, Default::default()).unwrap();
    let layout = game.open_stream(zone).unwrap().layout().clone();
    assert_eq!(layout.resources.len(), 4);
    assert_eq!(
        layout.resources[3].header.type_id, TYPE_SIMGROUP,
        "the clones go before the SimGroup"
    );
}

#[test]
fn breaking_objects_need_models_with_destructible_states() {
    use crate::resource::simgroup::{Attr, Packet, SimGroup, Value};
    let mut group = SimGroup::parse(&crate::resource::simgroup::tests::sample()).unwrap();
    let flags = |value: i32| Attr {
        index: 9,
        name: Some("CMD_SetThrowableFlags"),
        raw: value as u32,
        value: Value::Int(value),
        resource: None,
        target: None,
    };
    group.packets.push(Packet {
        offset: 0,
        class: Hash32(0x0a70_39c8),
        class_name: Some("Destructible"),
        attr_count: 20,
        attrs: vec![flags(64)],
    });
    let packet = group.packets.len() - 1;
    group.objects[0].packets.push(packet);
    let mut plain = empty(0x3e3f_49de);
    Fake::part(&mut plain, ("State", "State"), "ModelPart", Vec::new());
    let mut breaking = empty(0xc6a4_f42c);
    Fake::part(
        &mut breaking,
        ("DestructibleState", "destructible1"),
        "ModelPart",
        Vec::new(),
    );

    // A chandelier showing an ashtray: the crash of game-bench runs C and D.
    let problem = compat::model_problem(&group, 0, &plain).unwrap();
    assert!(problem.contains("CMD_SetThrowableFlags=64"), "{problem}");
    assert!(problem.contains("3e3f49de"), "{problem}");
    assert_eq!(compat::model_problem(&group, 0, &breaking), None);
    // Throwable flag 128 is what the game's plain destructibles have.
    group.packets[packet].attrs[0] = flags(128);
    assert_eq!(compat::model_problem(&group, 0, &plain), None);
    // Without a Destructible packet any model goes.
    group.packets[packet].attrs[0] = flags(64);
    group.objects[0].packets.pop();
    assert_eq!(compat::model_problem(&group, 0, &plain), None);
}

#[test]
fn a_new_stream_gets_a_simgroup_with_objects_and_a_cloned_model() {
    use super::edit::{self, GroupAt, ModelChoice};
    let (dir, game) = zone_game();
    let index = game.build_index(|_| {}).0;
    let zone = game.find_stream("gf2/zone").unwrap();
    let group = Location {
        stream: zone,
        resource: 1,
    };
    let mm_at = Location {
        stream: zone,
        resource: 0,
    };
    let mut ws =
        crate::workspace::Workspace::for_game(&game, Some(&dir.path().join("ws"))).unwrap();
    let none = |_: &Guid| false;
    let stream =
        crate::create::new_stream(&game, &ws, "gf2/doctor/area", "gf2/zone", false, &[], None)
            .unwrap();
    assert_eq!(stream.tocs, ["gf2/zone.toc"]);
    ws.add_stream(&game, stream).unwrap();

    // Not a stream of the game nor of the workspace.
    assert!(edit::add_group(&game, &index, &mut ws, "gf2/nowhere", None, &none).is_err());
    let empty = edit::add_group(&game, &index, &mut ws, "gf2/doctor/area", None, &none).unwrap();
    assert_eq!(empty.objects, 0);
    let at = GroupAt::Added(empty.key.clone());
    assert!(
        edit::current_group_at(&game, &ws, &at)
            .unwrap()
            .objects
            .is_empty()
    );

    // An object copied from the zone, showing a clone of the zone's model:
    // the clone goes into the new stream.
    let added = edit::add_at(
        &game,
        &index,
        &mut ws,
        &at,
        (&GroupAt::Game(group), 0),
        ModelChoice::Clone(mm_at),
        None,
        &none,
        true,
    )
    .unwrap();
    assert_eq!(added.object, 0);
    assert!(added.cloned.iter().all(|k| k.stream == "gf2/doctor/area"));
    let (copy, _) = edit::duplicate_at(&game, &mut ws, &at, 0, [1.0, 0.0, 0.0], &none).unwrap();
    assert_eq!(copy, 1);
    assert_eq!(edit::remove_at(&game, &mut ws, &at, 1).unwrap(), 0);
    let g = edit::current_group_at(&game, &ws, &at).unwrap();
    assert_eq!(g.objects.len(), 1);
    assert_eq!(g.model(&g.objects[0]), added.model);

    // A copy of the zone's group: new GUIDs.
    let copied = edit::add_group(
        &game,
        &index,
        &mut ws,
        "gf2/doctor/area",
        Some((&GroupAt::Game(group), None)),
        &none,
    )
    .unwrap();
    assert_eq!(copied.objects, 1);
    assert_ne!(copied.guids[0].0, copied.guids[0].1);

    // The file holds the model before the SimGroups made earlier.
    let order: Vec<u32> = ws
        .new_stream_resources("gf2/doctor/area")
        .iter()
        .map(|c| c.key.type_id)
        .collect();
    assert_eq!(order.first(), Some(&TYPE_METAMODEL));
    assert_eq!(order[order.len() - 2..], [TYPE_SIMGROUP, TYPE_SIMGROUP]);
    ws.apply(&game, Default::default()).unwrap();
    let written = std::fs::read(dir.path().join("gf2/doctor/area.str")).unwrap();
    let layout = crate::container::ustream::StreamLayout::parse(&written).unwrap();
    let types: Vec<u32> = layout.resources.iter().map(|r| r.header.type_id).collect();
    assert_eq!(types, order);

    // Undo takes back the copied group, then the removal.
    ws.undo().unwrap();
    assert_eq!(
        ws.new_stream_resources("gf2/doctor/area").len(),
        order.len() - 1
    );
}

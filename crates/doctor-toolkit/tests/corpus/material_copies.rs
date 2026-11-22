//! The rule of `navigation::placement` in the checks, and edits of a
//! material that reach every copy (`docs/analysis/resource-placement.md`
//! §5.5, §6). Reads the game; the edits go to workspaces in temporary
//! folders and are never applied.

use std::collections::{BTreeMap, HashMap};

use doctor_toolkit::create::{
    self, OrderItem, ReferenceChecker, TYPE_MATERIAL, TYPE_MESH, TYPE_SIMGROUP, Target,
};
use doctor_toolkit::game::GameInstance;
use doctor_toolkit::navigation::GameIndex;
use doctor_toolkit::navigation::placement::Placement;
use doctor_toolkit::resource::material::{self, Material, ParamEdit, ParamEditValue, ParamValue};
use doctor_toolkit::validate;
use doctor_toolkit::workspace::{
    NewResourceEdit, ResourceEdit, ResourceKey, StorageChoice, Workspace,
};

use super::game_dir;

/// Over the whole game, references resolve in the stream or in a stream
/// loaded with it, now with the streams the SimGroup data loads: 54 918
/// references stay unresolved, all texture references of materials to 5
/// textures no stream holds (the engine's own). With prerequisites alone
/// there were 55 034: 54 964 texture references to 26 textures and 70
/// models of SimGroup objects. No mesh names a material it does not
/// reach, and no mesh comes before its own material. The streams the
/// checker sees loaded by data are those of `Placement::requests` for
/// every stream.
#[test]
fn references_resolve_with_the_streams_data_loads() {
    let Some(dir) = game_dir() else { return };
    let game = GameInstance::open(&dir).unwrap();
    let mut checker = ReferenceChecker::new(&game).unwrap();
    let mut by_kind: BTreeMap<&str, usize> = BTreeMap::new();
    let mut textures = std::collections::BTreeSet::new();
    for s in 0..game.streams().len() {
        for u in checker.check(s).unwrap() {
            let kind = match (u.type_id, u.target, u.after) {
                (_, _, Some(_)) => "material after mesh",
                (TYPE_MATERIAL, Target::Texture(h), None) => {
                    textures.insert(h);
                    "material texture"
                }
                (TYPE_SIMGROUP, Target::Guid(_), None) => "simgroup model",
                _ => panic!("unexpected unresolved reference {u:?}"),
            };
            *by_kind.entry(kind).or_default() += 1;
        }
    }
    eprintln!("unresolved: {by_kind:?}, {} texture hashes", textures.len());
    assert_eq!(by_kind.get("material texture"), Some(&54_918));
    assert_eq!(by_kind.get("simgroup model"), None);
    assert_eq!(by_kind.get("material after mesh"), None);
    assert_eq!(textures.len(), 5);

    // One rule: what the checker reads from SimGroups without the index is
    // what the placement reads with it.
    let index = GameIndex::build(&game, &|_, _| {});
    let placement = Placement::new(&index, &game.stream_parents().unwrap());
    for s in 0..index.stream_count() {
        assert_eq!(
            checker.loads(s).unwrap(),
            placement.requests(s),
            "{}",
            index.stream_name(s)
        );
    }
}

/// In every stream of the game that holds materials and meshes, a material
/// the stream holds comes before every mesh of the stream that names it,
/// whatever other streams hold.
#[test]
fn no_mesh_comes_before_its_own_material() {
    let Some(dir) = game_dir() else { return };
    let game = GameInstance::open(&dir).unwrap();
    let mut both = 0;
    for s in 0..game.streams().len() {
        let stream = game.open_stream(s).unwrap();
        let items: Vec<OrderItem> = stream
            .layout()
            .resources
            .iter()
            .enumerate()
            .map(|(r, res)| match res.header.type_id {
                TYPE_MATERIAL => {
                    let data = stream.resource_data(r).unwrap();
                    OrderItem::Material(u32::from_le_bytes(data[8..12].try_into().unwrap()))
                }
                TYPE_MESH => {
                    let data = stream.resource_data(r).unwrap();
                    let targets = create::references(TYPE_MESH, &data).unwrap();
                    OrderItem::Mesh(create::material_targets(&targets))
                }
                _ => OrderItem::Other,
            })
            .collect();
        let has = |f: fn(&OrderItem) -> bool| items.iter().any(f);
        if has(|i| matches!(i, OrderItem::Material(_))) && has(|i| matches!(i, OrderItem::Mesh(_)))
        {
            both += 1;
        }
        let found = create::materials_after_meshes(&items, |_| false);
        assert!(
            found.is_empty(),
            "{}: {found:?}",
            game.streams()[s].stream_name()
        );
    }
    assert_eq!(both, 1_615);
}

/// The data of every copy of material `hash` as the workspace has them.
fn copy_data(game: &GameInstance, ws: &Workspace, hash: u32) -> Vec<(String, Vec<u8>)> {
    material::copies(game, ws, hash)
        .unwrap()
        .into_iter()
        .map(|t| (t.key().to_string(), t.current(game, ws).unwrap()))
        .collect()
}

/// An edit of a material goes to every copy of it (every material of the
/// game with its name hash) as one operation; with `this_stream_only` to
/// one, and `check-edits` then warns that the other copies keep other data.
#[test]
fn a_material_edit_reaches_every_copy() {
    let Some(dir) = game_dir() else { return };
    let game = GameInstance::open(&dir).unwrap();
    let (index, _) = game.build_index(|_| {});
    // A material with a few copies of the same data and a number parameter.
    let copies = material::game_copies(&game);
    let mut hashes: Vec<&u32> = copies.keys().collect();
    hashes.sort();
    let (hash, param, count) = hashes
        .into_iter()
        .filter(|h| (3..=6).contains(&copies[*h].len()))
        .find_map(|&h| {
            let datas: Vec<Vec<u8>> = copies[&h]
                .iter()
                .map(|&(s, r)| game.open_stream(s).unwrap().resource_data(r).unwrap())
                .collect();
            if datas.iter().any(|d| d != &datas[0]) {
                return None;
            }
            let m = Material::parse(&datas[0]).ok()?;
            let p = m
                .params
                .iter()
                .find(|p| matches!(&p.value, ParamValue::Floats(v) if !v.is_empty()))?;
            let ParamValue::Floats(v) = &p.value else {
                return None;
            };
            Some((h, p.name.clone(), v.len()))
        })
        .expect("a material with copies");
    let (stream, resource) = copies[&hash][0];
    let values: Vec<f32> = (0..count).map(|i| 0.25 + 0.125 * i as f32).collect();
    let edit = ParamEdit {
        name: param.clone(),
        value: ParamEditValue::Floats(values.clone()),
    };
    let has_values =
        |data: &[u8]| Material::parse(data).unwrap().floats(&param) == Some(&values[..]);

    // Every copy.
    let tmp = tempfile::tempdir().unwrap();
    let mut ws = Workspace::create(tmp.path().join("ws"), &game).unwrap();
    let done = material::set_params(
        &game,
        &mut ws,
        stream,
        resource,
        std::slice::from_ref(&edit),
        true,
    )
    .unwrap();
    eprintln!(
        "{}: {} of {} copies",
        done.label,
        done.changed_copies.len(),
        done.copies.len()
    );
    assert_eq!(done.copies.len(), copies[&hash].len());
    assert_eq!(done.changed_copies.len(), done.copies.len());
    assert!(done.skipped.is_empty());
    assert_eq!(ws.operations().len(), 1, "one operation");
    for (key, data) in copy_data(&game, &ws, hash) {
        assert!(has_values(&data), "{key}");
    }
    let report = validate::validate(&game, &index, &ws, &validate::Params::default()).unwrap();
    assert!(
        !report
            .warnings
            .iter()
            .any(|w| w.what.starts_with("copies of")),
        "{:?}",
        report.warnings
    );

    // One copy: the others keep the old values, and the check says so.
    let tmp = tempfile::tempdir().unwrap();
    let mut ws = Workspace::create(tmp.path().join("ws"), &game).unwrap();
    let done = material::set_params(&game, &mut ws, stream, resource, &[edit], false).unwrap();
    assert_eq!(done.copies.len(), 1);
    assert_eq!(done.changed_copies.len(), 1);
    let changed: usize = copy_data(&game, &ws, hash)
        .iter()
        .filter(|(_, d)| has_values(d))
        .count();
    assert_eq!(changed, 1);
    let report = validate::validate(&game, &index, &ws, &validate::Params::default()).unwrap();
    let warning = report
        .warnings
        .iter()
        .find(|w| w.what.starts_with("copies of"))
        .expect("a warning about the copies");
    eprintln!("{}: {}", warning.place(), warning.what);
    assert!(warning.what.contains(&format!(
        "in {} of {} stream(s)",
        copies[&hash].len() - 1,
        copies[&hash].len()
    )));
    assert!(report.ok, "a warning, not an error: {:?}", report.errors);
}

/// `check-edits` on a stream the workspace changes: a new material placed
/// after the mesh that names it is an error, placed before the mesh it is
/// not; either way the reference resolves.
#[test]
fn check_edits_wants_a_new_material_before_its_mesh() {
    let Some(dir) = game_dir() else { return };
    let game = GameInstance::open(&dir).unwrap();
    let (index, _) = game.build_index(|_| {});
    let mut checker = ReferenceChecker::new(&game).unwrap();
    // A small stream with a mesh whose first material only that stream has.
    let mut candidates: HashMap<usize, u64> = HashMap::new();
    for e in &index.entries {
        *candidates.entry(e.stream as usize).or_default() += u64::from(e.header.data_size);
    }
    let mut streams: Vec<(usize, u64)> = candidates.into_iter().collect();
    streams.sort_by_key(|&(s, size)| (size, s));
    let (stream, mesh, material_at) = streams
        .iter()
        .find_map(|&(s, _)| {
            let file = game.open_stream(s).ok()?;
            let resources = &file.layout().resources;
            let mut materials: HashMap<u32, usize> = HashMap::new();
            for (r, res) in resources.iter().enumerate() {
                if res.header.type_id == TYPE_MATERIAL {
                    let d = file.resource_data(r).ok()?;
                    materials.insert(u32::from_le_bytes(d[8..12].try_into().ok()?), r);
                }
            }
            for (r, res) in resources.iter().enumerate() {
                if res.header.type_id != TYPE_MESH {
                    continue;
                }
                let d = file.resource_data(r).ok()?;
                let targets = create::references(TYPE_MESH, &d).ok()?;
                let first = *create::material_targets(&targets).first()?;
                if let Some(&at) = materials.get(&first)
                    && !checker
                        .provided_elsewhere(s, &Target::Material(first))
                        .unwrap()
                {
                    return Some((s, r, at));
                }
            }
            None
        })
        .expect("a stream with a mesh and its own material");
    let file = game.open_stream(stream).unwrap();
    let name = game.streams()[stream].stream_name();
    let mesh_key = ResourceKey::new(&name, &file.layout().resources[mesh].header);
    let mesh_data = file.resource_data(mesh).unwrap();
    let old = u32::from_le_bytes(
        create::references(TYPE_MESH, &mesh_data)
            .unwrap()
            .iter()
            .find_map(|t| match t {
                Target::Material(h) => Some(*h),
                _ => None,
            })
            .unwrap()
            .to_le_bytes(),
    );
    // The material under a new name, the mesh pointed at it.
    let new_hash = doctor_toolkit::hash::crc32(b"doctor_order_test_material");
    let mut material = file.resource_data(material_at).unwrap();
    material[8..12].copy_from_slice(&new_hash.to_le_bytes());
    material[12..16].copy_from_slice(&new_hash.to_le_bytes());
    let mut new_mesh = mesh_data.clone();
    let count = u32::from_le_bytes(mesh_data[0x34..0x38].try_into().unwrap()) as usize;
    let records = u32::from_le_bytes(mesh_data[0x50..0x54].try_into().unwrap()) as usize;
    for i in 0..count {
        let at = records + 0xB0 * i + 0x0C;
        if new_mesh[at..at + 4] == old.to_le_bytes() {
            new_mesh[at..at + 4].copy_from_slice(&new_hash.to_le_bytes());
        }
    }
    let ids = [0xD0C7_0A11, 0x0BDE_A001, 0x5EED_0001, new_hash];
    assert!(!index.entries.iter().any(|e| e.header.ids == ids));
    let header = file.layout().resources[material_at].header;
    // `into`: the stream the new material goes to.
    let record = |into: &str, before: Option<ResourceKey>| -> Vec<validate::Finding> {
        let tmp = tempfile::tempdir().unwrap();
        let mut ws = Workspace::create(tmp.path().join("ws"), &game).unwrap();
        ws.change_resources(
            &game,
            "material after or before its mesh",
            &[(
                stream,
                vec![ResourceEdit {
                    index: mesh,
                    data: new_mesh.clone(),
                    storage: StorageChoice::Keep,
                }],
            )],
            &[NewResourceEdit {
                stream: into.to_owned(),
                header: doctor_toolkit::container::ustream::ResourceHeader { ids, ..header },
                data: material.clone(),
                storage: StorageChoice::Keep,
                before,
            }],
        )
        .unwrap();
        validate::validate(&game, &index, &ws, &validate::Params::default())
            .unwrap()
            .errors
    };
    let resolves = |errors: &[validate::Finding]| {
        !errors
            .iter()
            .any(|e| e.what.starts_with("unresolved reference"))
    };
    let after = record(&name, None);
    assert!(resolves(&after), "{after:?}");
    eprintln!("{after:?}");
    let error = after
        .iter()
        .find(|e| e.what.contains("must come before the mesh"))
        .expect("an error about the order");
    assert_eq!(error.key.as_deref(), Some(mesh_key.to_string().as_str()));
    let before = record(&name, Some(mesh_key.clone()));
    assert!(resolves(&before), "{before:?}");
    assert!(
        !before
            .iter()
            .any(|e| e.what.contains("must come before the mesh")),
        "{before:?}"
    );
    // In a stream the mesh's stream does not reach: the reference does not
    // resolve, and the check names the stream a copy is needed in.
    let reach = checker.reach(stream).unwrap();
    let other = (0..game.streams().len())
        .find(|s| *s != stream && !reach.contains(s))
        .map(|s| game.streams()[s].stream_name())
        .unwrap();
    let elsewhere = record(&other, None);
    let error = elsewhere
        .iter()
        .find(|e| e.what.starts_with("unresolved reference"))
        .expect("an unresolved reference");
    eprintln!("{}: {}", error.place(), error.what);
    assert_eq!(error.key.as_deref(), Some(mesh_key.to_string().as_str()));
    assert!(
        error
            .what
            .ends_with(&format!("it is in {other}: a copy is needed in {name}")),
        "{}",
        error.what
    );
}

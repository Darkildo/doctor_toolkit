//! Mods of format version 6 on the miniature game ([`crate::testing`]):
//! building deltas, folding them in the mod manager, the checks and the
//! conflicts by operation.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use super::super::manager::{CompatIssue, ModApplyOptions, Problem};
use super::super::package::ModArchive;
use super::super::*;
use super::FOLD_DIR;
use crate::container::ustream::{Replacement, Storage};
use crate::resource::delta::{Delta, Kind};
use crate::resource::lh2::StringTable;
use crate::resource::simgroup::{CLASS_ENTITY, Guid, SimGroup, Value};
use crate::resource::wgi::{self, NewCell, WorldGrid};
use crate::testing::{self, LEVEL, STRINGS, TOWN, YARD, YARD_SIMGROUP};
use crate::workspace::PayloadStore;
use crate::workspace::tests::{edit, game_data, key, snapshot, stream};

/// Index of the world grid in [`TOWN`].
const GRID: usize = 0;
/// Crates the yard of [`town`] has besides the one of the miniature game.
const CRATES: u8 = 12;

pub(crate) fn guid(n: u8) -> Guid {
    let mut bytes = *b"\0\0\0X\0\0\0\0DOCTORTK";
    bytes[0] = n;
    Guid(bytes)
}

fn stream_guid(seed: u8) -> [u8; 16] {
    let mut guid = *b"\0\0\0Z\0\0\0\0DOCTORTK";
    guid[1] = seed;
    guid
}

fn zone(seed: u8) -> NewCell {
    NewCell {
        guid: stream_guid(seed),
        box_min: [f32::from(seed) * 30.0, 0.0, -4.0],
        box_max: [f32::from(seed) * 30.0 + 20.0, 6.0, 4.0],
        kind: wgi::KIND_WORLD,
        flags: 0,
        lod_mask: wgi::lod::HIGH,
        name: Some(format!("Zone_{seed}")),
        needs: None,
    }
}

/// The miniature game with its town, and resources large enough that what
/// a mod does to them is smaller than they are: a yard of thirteen crates
/// (`guid(1)` to `guid(12)` besides the game's), a world grid of nine zones
/// and a string table of forty-two strings.
pub(crate) fn town() -> (tempfile::TempDir, GameInstance) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_owned();
    testing::write_game(&root).unwrap();
    testing::write_town(&root).unwrap();
    let game = GameInstance::open(&root).unwrap();
    let replace = |name: &str, index: usize, data: Vec<u8>| {
        let replacement = Replacement {
            index,
            data: &data,
            storage: Storage::RefPack,
        };
        game.replace_resources(stream(&game, name), &[replacement], Default::default())
            .unwrap();
    };
    let yard = edit_group(&game_data(&game, YARD, YARD_SIMGROUP), |group| {
        for n in 1..=CRATES {
            let copy = group.duplicate_object(0, guid(n)).unwrap();
            group
                .set_position(copy, [10.0 * f32::from(n), 0.0, 1.0])
                .unwrap();
            let name = Value::String(format!("crate_{n}"));
            group
                .set_class_attr(copy, CLASS_ENTITY, 2, Some(name))
                .unwrap();
        }
    });
    replace(YARD, YARD_SIMGROUP, yard);
    let grid = edit_grid(&game_data(&game, TOWN, GRID), |grid| {
        for seed in 1..=8 {
            grid.add_cell(&zone(seed)).unwrap();
        }
    });
    replace(TOWN, GRID, grid);
    let table = edit_table(&game_data(&game, LEVEL, STRINGS), |table| {
        for n in 0..40u32 {
            table
                .insert(0x100 + n, &[format!("The line number {n} of the game")])
                .unwrap();
        }
    });
    replace(LEVEL, STRINGS, table);
    // The game as it is now is the unmodified game.
    fs::remove_dir_all(root.join("_doctor_toolkit")).unwrap();
    (dir, GameInstance::open(&root).unwrap())
}

pub(crate) fn edit_group(data: &[u8], change: impl FnOnce(&mut SimGroup)) -> Vec<u8> {
    let mut group = SimGroup::parse(data).unwrap();
    change(&mut group);
    group.encode()
}

fn edit_grid(data: &[u8], change: impl FnOnce(&mut WorldGrid)) -> Vec<u8> {
    let mut grid = WorldGrid::parse(data).unwrap();
    change(&mut grid);
    grid.to_bytes().unwrap()
}

pub(crate) fn edit_table(data: &[u8], change: impl FnOnce(&mut StringTable)) -> Vec<u8> {
    let mut table = StringTable::parse(data).unwrap();
    change(&mut table);
    table.to_bytes().unwrap()
}

/// Sets the text of string `hash` in the first column.
pub(crate) fn set_text(table: &mut StringTable, hash: u32, text: &str) {
    let row = table.find(hash).unwrap();
    table.strings[0][row] = text.to_owned();
}

pub(crate) fn object(group: &SimGroup, n: u8) -> usize {
    let guid = guid(n);
    group
        .objects
        .iter()
        .position(|o| o.guid == guid)
        .unwrap_or_else(|| panic!("no object {n}"))
}

/// Adds a crate `guid(n)` next to the first one.
pub(crate) fn add_crate(group: &mut SimGroup, n: u8) {
    let copy = group.duplicate_object(0, guid(n)).unwrap();
    group
        .set_position(copy, [-10.0 * f32::from(n), 0.0, 2.0])
        .unwrap();
}

/// Sets the attribute the tests use for conflicts (`Entity.CMD_SetInteriorLit`).
fn light(group: &mut SimGroup, n: u8, value: i32) {
    let at = object(group, n);
    group
        .set_class_attr(at, CLASS_ENTITY, 3, Some(Value::Int(value)))
        .unwrap();
}

fn lit(group: &SimGroup, n: u8) -> Option<Value> {
    group
        .attr(&group.objects[object(group, n)], CLASS_ENTITY, 3)
        .cloned()
}

pub(crate) fn info(id: &str, version: &str) -> ModInfo {
    ModInfo {
        id: id.to_owned(),
        name: format!("Mod {id}"),
        version: version.to_owned(),
        ..ModInfo::default()
    }
}

/// A workspace in `dir` that edits resource `index` of `name` with `change`.
pub(crate) fn workspace(
    game: &GameInstance,
    dir: &Path,
    name: &str,
    index: usize,
    change: impl FnOnce(&[u8]) -> Vec<u8>,
) -> Workspace {
    let mut ws = Workspace::create(dir, game).unwrap();
    let data = change(&game_data(game, name, index));
    ws.set_resources(game, "edit", stream(game, name), &[edit(index, data)])
        .unwrap();
    ws
}

/// Builds the mod `id` that edits the yard of the game as it is with `change`.
pub(crate) fn yard_mod(
    game: &GameInstance,
    temp: &Path,
    info: &ModInfo,
    change: impl FnOnce(&mut SimGroup),
) -> PathBuf {
    let dir = temp.join(format!("ws-{}-{}", info.id, info.version));
    let ws = workspace(game, &dir, YARD, YARD_SIMGROUP, |data| {
        edit_group(data, change)
    });
    let out = temp.join(format!("{}-{}.zip", info.id, info.version));
    build_from_workspace(&ws, info, None, None, &out).unwrap();
    out
}

fn manifest_of(path: &Path) -> ModManifest {
    let mut archive = ModArchive::open(path).unwrap();
    archive.verify().unwrap();
    archive.manifest().clone()
}

/// The yard as the game files have it now.
fn yard_now(game: &GameInstance) -> SimGroup {
    let data = game_data(game, YARD, YARD_SIMGROUP);
    let group = SimGroup::parse(&data).unwrap();
    assert_eq!(group.encode(), data, "the group is written as it is read");
    group
}

pub(crate) fn has(group: &SimGroup, n: u8) -> bool {
    group.objects.iter().any(|o| o.guid == guid(n))
}

fn problems<'a>(issues: &'a [CompatIssue], id: &str) -> Vec<&'a Problem> {
    issues
        .iter()
        .filter(|i| i.mod_id == id)
        .map(|i| &i.problem)
        .collect()
}

fn apply(mods: &mut ModManager, game: &GameInstance) -> crate::workspace::ApplyReport {
    mods.apply(game, ModApplyOptions::default()).unwrap()
}

// ------------------------------------------------------------ the format

#[test]
fn version_6_manifests_round_trip_and_older_managers_refuse_them() {
    let (_dir, game) = town();
    let temp = tempfile::tempdir().unwrap();
    let path = yard_mod(&game, temp.path(), &info("a", "1.0"), |g| add_crate(g, 21));
    let manifest = manifest_of(&path);
    assert_eq!(manifest.format_version, DELTA_VERSION);
    assert_eq!(manifest.required_version(), 6);
    let resource = &manifest.resources[0];
    assert_eq!(
        resource.delta,
        Some(ModDelta {
            kind: Kind::SimGroup,
            format: 1
        })
    );
    // The payload is the delta document, the original what it was made on.
    let yard = game_data(&game, YARD, YARD_SIMGROUP);
    assert_eq!(resource.original, Blob::of(&yard));
    let mut archive = ModArchive::open(&path).unwrap();
    let document = archive.read_payload(&resource.payload).unwrap();
    let delta = Delta::parse(&document).unwrap();
    assert_eq!((delta.kind(), delta.len()), (Kind::SimGroup, 1));
    assert!(document.len() < yard.len());
    assert_eq!(
        archive.delta_operations().unwrap(),
        [(resource.key.clone(), 1)]
    );
    let file = file_info(&path).unwrap();
    assert_eq!(
        (file.format_version, file.deltas, file.operations),
        (6, 1, 1)
    );

    // Written and read back, the manifest and its bytes are the same.
    let json = manifest.to_json().unwrap();
    let text = String::from_utf8(json.clone()).unwrap();
    assert!(
        text.contains(r#""delta": {"#) && text.contains(r#""kind": "simgroup""#),
        "{text}"
    );
    let again = ModManifest::parse(&json).unwrap();
    assert_eq!(again, manifest);
    assert_eq!(again.to_json().unwrap(), json);

    // A manager that reads up to version 5 says "update" and installs
    // nothing: it would have written the document into the game.
    let error = ModManifest::parse_up_to(&json, NATIVE_VERSION).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("format version 6; this program reads up to 5. Update Open Mod Manager"),
        "{error}"
    );

    // The field needs the version: a manifest of an older one is refused.
    let refused = |change: &dyn Fn(&mut ModManifest)| {
        let mut bad = manifest.clone();
        change(&mut bad);
        bad.validate().unwrap_err().to_string()
    };
    let error = refused(&|m| m.format_version = 5);
    assert!(error.contains("version 6 is needed"), "{error}");
    let error = refused(&|m| m.resources[0].delta.as_mut().unwrap().kind = Kind::Lh2);
    assert!(error.contains("a lh2 delta does not fit"), "{error}");
    let error = refused(&|m| m.resources[0].delta.as_mut().unwrap().format = 2);
    assert!(error.contains("Update Open Mod Manager"), "{error}");
    let error = refused(&|m| {
        m.resources[0].added = Some(crate::workspace::Added {
            mem_policy: 1,
            alignment: 16,
            before: None,
            order: 1,
        });
        m.resources[0].original = Blob::of(&[]);
    });
    assert!(error.contains("cannot be a delta"), "{error}");

    // A delta may leave `original` out; a whole resource may not.
    let mut unsaid = manifest.clone();
    unsaid.resources[0].original = Blob::of(&[]);
    let json = unsaid.to_json().unwrap();
    assert!(!String::from_utf8_lossy(&json).contains("\"original\""));
    assert_eq!(ModManifest::parse(&json).unwrap(), unsaid);
    let whole = String::from_utf8(json)
        .unwrap()
        .replace(r#""kind": "simgroup""#, r#""kind": "simgroup", "x": 1"#);
    assert!(
        ModManifest::parse(whole.as_bytes()).is_ok(),
        "unknown fields are skipped"
    );
    let mut value: serde_json::Value =
        serde_json::from_slice(&manifest.to_json().unwrap()).unwrap();
    let entry = value["resources"][0].as_object_mut().unwrap();
    entry.remove("delta");
    entry.remove("original");
    value["format_version"] = 3.into();
    let error = ModManifest::parse(&serde_json::to_vec(&value).unwrap()).unwrap_err();
    assert!(
        error.to_string().contains("missing field `original`"),
        "{error}"
    );

    // A payload that is not the delta the manifest names is refused when
    // the file is read, not when the game is written.
    let mut manifest = manifest;
    let junk = br#"{"format":1,"kind":"lh2","ops":[]}"#.to_vec();
    manifest.resources[0].payload = Blob::of(&junk);
    let bad = temp.path().join("bad.zip");
    super::super::package::write_package(&bad, &manifest, None, |_| Ok(junk.clone())).unwrap();
    let error = file_info(&bad).unwrap_err().to_string();
    assert!(
        error.contains("a lh2 delta, which is not what mod.json says"),
        "{error}"
    );
    let mut mods = ModManager::open(&game).unwrap();
    assert!(mods.install_file(&game, &bad, true).is_err());
    assert!(mods.mods().is_empty());
}

#[test]
fn the_build_keeps_a_resource_whole_when_a_delta_is_not_exact_or_not_smaller() {
    let (_dir, game) = town();
    let temp = tempfile::tempdir().unwrap();
    let yard = game_data(&game, YARD, YARD_SIMGROUP);

    // Asked to: every resource whole, in the version the mod had before
    // deltas, and nothing in the manifest that an older manager skips.
    let whole = ModInfo {
        whole_resources: true,
        ..info("whole", "1.0")
    };
    let path = yard_mod(&game, temp.path(), &whole, |g| add_crate(g, 21));
    let manifest = manifest_of(&path);
    assert_eq!(manifest.format_version, RELATIONS_VERSION);
    assert_eq!(manifest.deltas().count(), 0);
    assert_eq!(manifest.resources[0].original, Blob::of(&yard));
    assert!(
        !String::from_utf8(manifest.to_json().unwrap())
            .unwrap()
            .contains("delta")
    );
    ModManifest::parse_up_to(&manifest.to_json().unwrap(), RELATIONS_VERSION).unwrap();
    let old = ModInfo {
        relations: Relations::none(),
        ..ModInfo {
            whole_resources: true,
            ..info("old", "1.0")
        }
    };
    let manifest = manifest_of(&yard_mod(&game, temp.path(), &old, |g| add_crate(g, 21)));
    assert_eq!(manifest.format_version, 1);

    // A change that is no operation (the flags of an object): whole.
    let dir = temp.path().join("ws-flags");
    let ws = workspace(&game, &dir, YARD, YARD_SIMGROUP, |data| {
        edit_group(data, |g| g.objects[1].flags2 ^= 1)
    });
    let built = mod_from_workspace(&ws, &info("flags", "1.0"), None).unwrap();
    assert_eq!(built.manifest.format_version, RELATIONS_VERSION);
    assert_eq!(built.kept_whole.len(), 1);
    assert_eq!(built.kept_whole[0].1, KeptWhole::NotExact);
    assert_eq!(
        built.manifest.resources[0].payload,
        ws.change(&key(&game, YARD, YARD_SIMGROUP)).unwrap().payload
    );

    // A delta that takes no less room in the mod file than the resource:
    // the mod changes most of it. The small group of the level gets as
    // many objects again as it has.
    let small = game_data(&game, LEVEL, testing::SIMGROUP);
    let dir = temp.path().join("ws-many");
    let ws = workspace(&game, &dir, LEVEL, testing::SIMGROUP, |data| {
        edit_group(data, |g| {
            for n in 0..g.objects.len() {
                g.duplicate_object(n, guid(70 + n as u8)).unwrap();
            }
        })
    });
    let mine = ws
        .store()
        .get(&ws.changes().next().unwrap().payload)
        .unwrap();
    let made = delta_for(key(&game, LEVEL, testing::SIMGROUP).type_id, &small, &mine);
    let Err(KeptWhole::NotSmaller { delta, resource }) = made else {
        panic!(
            "{:?} of {}",
            made.map(|document| document.len()),
            mine.len()
        );
    };
    // The sizes are those in the mod file: both deflated.
    assert!(delta >= resource && resource < mine.len() as u64);
    let out = temp.path().join("many.zip");
    let report = build_from_workspace(&ws, &info("many", "1.0"), None, None, &out).unwrap();
    assert_eq!((report.deltas, report.delta_kinds), (0, 1));
    assert_eq!(report.format_version, RELATIONS_VERSION);
    assert!(
        report
            .notes
            .iter()
            .any(|n| n.contains("is in the mod whole") && n.contains("the mod changes most of it")),
        "{:?}",
        report.notes
    );

    // A mod of a mesh and a delta: the mesh is whole, the report counts.
    let dir = temp.path().join("ws-mixed");
    let mut ws = workspace(&game, &dir, YARD, YARD_SIMGROUP, |data| {
        edit_group(data, |g| add_crate(g, 21))
    });
    ws.set_resources(
        &game,
        "mesh",
        stream(&game, YARD),
        &[edit(testing::YARD_MESH, vec![7; 500])],
    )
    .unwrap();
    let out = temp.path().join("mixed.zip");
    let report = build_from_workspace(&ws, &info("mixed", "1.0"), None, None, &out).unwrap();
    assert_eq!(
        (report.resources, report.deltas, report.delta_kinds),
        (2, 1, 1)
    );
    assert_eq!(report.format_version, DELTA_VERSION);
    assert!(report.notes.is_empty(), "{:?}", report.notes);
}

/// The seed of the fuzz target `mod_manifest` for format version 6: every
/// kind of delta, with and without `original`, next to a whole resource.
#[test]
fn the_fuzz_seed_of_version_6_is_a_manifest_with_every_kind_of_delta() {
    let seed = include_bytes!("../../../../../fuzz/seeds/mod_manifest/manifest_v6.json");
    let manifest = ModManifest::parse(seed).unwrap();
    assert_eq!(manifest.format_version, 6);
    let kinds: Vec<Kind> = manifest.deltas().map(|r| r.delta.unwrap().kind).collect();
    assert_eq!(
        kinds,
        [Kind::SimGroup, Kind::Wgi, Kind::Lh2, Kind::PartsDb],
        "one of each"
    );
    assert_eq!(manifest.resources.len(), 5);
    let written = manifest.to_json().unwrap();
    assert!(ModManifest::parse(&written).unwrap() == manifest);
    assert!(ModManifest::parse_up_to(seed, 5).is_err());
}

// -------------------------------------------------------------- the fold

#[test]
fn two_mods_that_add_objects_to_one_simgroup_are_both_in_the_game() {
    let (dir, game) = town();
    let root = dir.path();
    let pristine = snapshot(root);
    let temp = tempfile::tempdir().unwrap();
    let a = yard_mod(&game, temp.path(), &info("a", "1.0"), |g| {
        add_crate(g, 21);
        light(g, 1, 4);
    });
    let b = yard_mod(&game, temp.path(), &info("b", "1.0"), |g| {
        add_crate(g, 22);
        g.set_position(object(g, 2), [1.0, 2.0, 3.0]).unwrap();
    });
    let mut mods = ModManager::open(&game).unwrap();
    for path in [&a, &b] {
        let report = mods.install_file(&game, path, true).unwrap();
        assert!(report.issues.is_empty(), "{:?}", report.issues);
    }
    assert_eq!(
        snapshot(root),
        pristine,
        "installing does not touch the game"
    );
    assert!(mods.check(&game).unwrap().is_empty());
    assert!(mods.conflicts(&game, true).unwrap().is_empty());
    assert!(mods.conflicts(&game, false).unwrap().is_empty());
    // Nothing is written by the checks: the folds are kept in memory.
    assert!(!mods.dir().join(FOLD_DIR).exists());

    let report = apply(&mut mods, &game);
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    let group = yard_now(&game);
    assert_eq!(group.objects.len(), usize::from(CRATES) + 3);
    assert!(has(&group, 21) && has(&group, 22));
    // In load order: a's crate, then b's.
    assert!(object(&group, 21) < object(&group, 22));
    assert_eq!(lit(&group, 1), Some(Value::Int(4)));
    let moved = &group.objects[object(&group, 2)];
    assert_eq!(group.transform(moved).unwrap()[3][..3], [1.0, 2.0, 3.0]);

    // The plan names the folded data, which is what the record and the
    // comparison with the game go by.
    let yard_key = key(&game, YARD, YARD_SIMGROUP);
    let written = Blob::of(&game_data(&game, YARD, YARD_SIMGROUP));
    let plan = mods.plan(&game).unwrap();
    assert_eq!(plan[&yard_key].0.payload, written);
    assert_eq!(plan[&yard_key].1, 1, "the last mod in it");
    let status = mods.status(&game).unwrap();
    assert!(status.streams.iter().all(|s| s.up_to_date));
    let record: crate::workspace::Deployment =
        serde_json::from_slice(&fs::read(mods.dir().join("applied.json")).unwrap()).unwrap();
    assert_eq!(record.streams[YARD].resources[&yard_key], written.sha256);
    // The fold is kept: its data by its hash, its two steps in the index.
    let folded = mods.dir().join(FOLD_DIR);
    let store = PayloadStore::new(folded.join(PAYLOAD_DIR));
    assert_eq!(store.get(&written).unwrap().len() as u64, written.size);
    let index: serde_json::Value =
        serde_json::from_slice(&fs::read(folded.join("index.json")).unwrap()).unwrap();
    assert_eq!(index["steps"].as_object().unwrap().len(), 2);
    // Nothing to do the second time.
    assert!(apply(&mut mods, &game).streams.is_empty());

    // A manager opened anew knows the steps: it reads no delta to compare
    // the mods with the game (the payloads of the mods are taken away).
    let hidden = temp.path().join("hidden");
    fs::rename(mods.get("a").unwrap().dir().join(PAYLOAD_DIR), &hidden).unwrap();
    let again = ModManager::open(&game).unwrap();
    assert!(
        again
            .status(&game)
            .unwrap()
            .streams
            .iter()
            .all(|s| s.up_to_date)
    );
    assert!(again.check(&game).unwrap().is_empty());
    fs::rename(&hidden, mods.get("a").unwrap().dir().join(PAYLOAD_DIR)).unwrap();

    // The other order folds anew: b's crate first.
    let both = snapshot(root);
    mods.move_mod("b", 0).unwrap();
    assert!(
        !mods
            .status(&game)
            .unwrap()
            .streams
            .iter()
            .all(|s| s.up_to_date)
    );
    apply(&mut mods, &game);
    let group = yard_now(&game);
    assert!(object(&group, 22) < object(&group, 21));
    assert_eq!(group.objects.len(), usize::from(CRATES) + 3);
    mods.move_mod("b", 1).unwrap();
    apply(&mut mods, &game);
    assert_eq!(
        snapshot(root),
        both,
        "the same mods in the same order, the same bytes"
    );

    // One mod off: the other alone, as its author's bytes (the layout of
    // its delta fits the game's own data).
    mods.set_enabled("a", false).unwrap();
    apply(&mut mods, &game);
    let group = yard_now(&game);
    assert!(!has(&group, 21) && has(&group, 22));
    assert_eq!(lit(&group, 1), None);
    let alone = edit_group(&pristine_yard(&pristine), |g| {
        add_crate(g, 22);
        g.set_position(object(g, 2), [1.0, 2.0, 3.0]).unwrap();
    });
    assert_eq!(game_data(&game, YARD, YARD_SIMGROUP), alone);
    mods.set_enabled("a", true).unwrap();
    apply(&mut mods, &game);
    assert_eq!(snapshot(root), both);

    // Restore originals, and taking the mods out, give the game back.
    mods.unapply(&game, false).unwrap();
    assert_eq!(snapshot(root), pristine);
    apply(&mut mods, &game);
    assert_eq!(snapshot(root), both);
    mods.uninstall("a").unwrap();
    mods.uninstall("b").unwrap();
    apply(&mut mods, &game);
    assert_eq!(snapshot(root), pristine);
    assert!(!folded.exists(), "no folds are kept for no mods");
}

/// The yard of a snapshot of the unmodified game.
fn pristine_yard(pristine: &BTreeMap<String, Vec<u8>>) -> Vec<u8> {
    let file = &pristine[&format!("{YARD}.str")];
    let layout = crate::container::ustream::StreamLayout::parse(file).unwrap();
    layout.resource_data(file, YARD_SIMGROUP).unwrap()
}

#[test]
fn two_mods_that_add_a_zone_to_one_world_grid_are_both_in_the_game() {
    let (dir, game) = town();
    let root = dir.path();
    let pristine = snapshot(root);
    let temp = tempfile::tempdir().unwrap();
    let mut mods = ModManager::open(&game).unwrap();
    for (id, seed) in [("north", 40u8), ("south", 41)] {
        let ws = workspace(&game, &temp.path().join(id), TOWN, GRID, |data| {
            edit_grid(data, |grid| {
                grid.add_cell(&zone(seed)).unwrap();
            })
        });
        let out = temp.path().join(format!("{id}.zip"));
        let report = build_from_workspace(&ws, &info(id, "1.0"), None, None, &out).unwrap();
        assert_eq!((report.deltas, report.format_version), (1, 6));
        assert_eq!(
            manifest_of(&out).resources[0].delta.unwrap().kind,
            Kind::Wgi
        );
        assert!(
            mods.install_file(&game, &out, true)
                .unwrap()
                .issues
                .is_empty()
        );
    }
    assert!(mods.conflicts(&game, true).unwrap().is_empty());
    apply(&mut mods, &game);
    let grid = WorldGrid::parse(&game_data(&game, TOWN, GRID)).unwrap();
    assert_eq!(grid.cells.len(), 11);
    for seed in [40, 41] {
        let hash = crate::toc::guid_hash(&stream_guid(seed));
        assert!(grid.find_cell(hash).is_some(), "the cell of zone {seed}");
    }
    mods.unapply(&game, false).unwrap();
    assert_eq!(snapshot(root), pristine);

    // Two mods that add the same zone cannot both be right: the later one
    // is stopped, by name, and nothing is written.
    let ws = workspace(&game, &temp.path().join("again"), TOWN, GRID, |data| {
        edit_grid(data, |grid| {
            grid.add_cell(&zone(40)).unwrap();
        })
    });
    let out = temp.path().join("again.zip");
    build_from_workspace(&ws, &info("again", "1.0"), None, None, &out).unwrap();
    let report = mods.install_file(&game, &out, true).unwrap();
    let [Problem::DeltaRefused { error }] = problems(&report.issues, "again")[..] else {
        panic!("{:?}", report.issues);
    };
    assert!(
        error.contains("the grid already has a cell of stream"),
        "{error}"
    );
    assert!(report.issues[0].is_fatal());
    let error = mods.apply(&game, ModApplyOptions::default()).unwrap_err();
    assert!(
        error.to_string().contains("again:") && error.to_string().contains("cannot be merged"),
        "{error}"
    );
    assert_eq!(snapshot(root), pristine);
    // The earlier mods are fine; without the later one they are written.
    let issues = mods.check(&game).unwrap();
    assert!(issues.iter().all(|i| i.mod_id == "again"), "{issues:?}");
    mods.set_enabled("again", false).unwrap();
    apply(&mut mods, &game);
    assert_eq!(
        WorldGrid::parse(&game_data(&game, TOWN, GRID))
            .unwrap()
            .cells
            .len(),
        11
    );
}

#[test]
fn two_mods_that_set_one_attribute_are_a_conflict_the_later_wins() {
    let (_dir, game) = town();
    let temp = tempfile::tempdir().unwrap();
    let a = yard_mod(&game, temp.path(), &info("a", "1.0"), |g| {
        light(g, 2, 5);
        light(g, 3, 1);
        add_crate(g, 21);
    });
    let b = yard_mod(&game, temp.path(), &info("b", "1.0"), |g| {
        light(g, 2, 7);
        light(g, 3, 1);
        light(g, 4, 9);
    });
    let mut mods = ModManager::open(&game).unwrap();
    mods.install_file(&game, &a, true).unwrap();
    mods.install_file(&game, &b, true).unwrap();
    // The mods merge; one value is set by both, differently (crate 3 gets
    // the same value from both, which is no conflict).
    let conflicts = mods.conflicts(&game, true).unwrap();
    assert_eq!(conflicts.len(), 1, "{conflicts:?}");
    let conflict = &conflicts[0];
    assert_eq!(conflict.key, key(&game, YARD, YARD_SIMGROUP));
    assert_eq!(conflict.mods, ["a", "b"]);
    assert_eq!(conflict.winner(), "b");
    assert!(!conflict.same_data && !conflict.patch);
    assert_eq!(conflict.elements.len(), 1);
    let element = &conflict.elements[0];
    assert!(
        element.contains(&format!("object {}", guid(2)))
            && element.contains("Entity.CMD_SetInteriorLit")
            && element.contains("the value was 5"),
        "{element}"
    );
    // A conflict is not a misfit: nothing to tell about either mod.
    assert!(mods.check(&game).unwrap().is_empty());
    apply(&mut mods, &game);
    let group = yard_now(&game);
    assert_eq!(lit(&group, 2), Some(Value::Int(7)));
    assert_eq!(lit(&group, 3), Some(Value::Int(1)));
    assert_eq!(lit(&group, 4), Some(Value::Int(9)));
    assert!(has(&group, 21));

    // The other order: the other mod wins, and the conflict says so.
    mods.move_mod("a", 1).unwrap();
    let conflicts = mods.conflicts(&game, true).unwrap();
    assert_eq!(conflicts.len(), 1);
    assert_eq!(
        (conflicts[0].mods.clone(), conflicts[0].winner()),
        (vec!["b".to_owned(), "a".to_owned()], "a")
    );
    assert!(conflicts[0].elements[0].contains("the value was 7"));
    apply(&mut mods, &game);
    assert_eq!(lit(&yard_now(&game), 2), Some(Value::Int(5)));

    // Only among the enabled mods, unless asked.
    mods.set_enabled("b", false).unwrap();
    assert!(mods.conflicts(&game, true).unwrap().is_empty());
    assert_eq!(mods.conflicts(&game, false).unwrap().len(), 1);
}

#[test]
fn a_whole_resource_above_a_delta_replaces_it() {
    let (_dir, game) = town();
    let temp = tempfile::tempdir().unwrap();
    let merging = yard_mod(&game, temp.path(), &info("merging", "1.0"), |g| {
        add_crate(g, 21)
    });
    let whole = ModInfo {
        whole_resources: true,
        ..info("whole", "1.0")
    };
    let whole = yard_mod(&game, temp.path(), &whole, |g| add_crate(g, 22));
    let mut mods = ModManager::open(&game).unwrap();
    mods.install_file(&game, &merging, true).unwrap();
    mods.install_file(&game, &whole, true).unwrap();
    // The conflict it always was: the last mod has the resource.
    let conflicts = mods.conflicts(&game, true).unwrap();
    assert_eq!(conflicts.len(), 1);
    assert_eq!(conflicts[0].mods, ["merging", "whole"]);
    assert!(conflicts[0].elements.is_empty() && !conflicts[0].same_data);
    assert!(mods.check(&game).unwrap().is_empty());
    apply(&mut mods, &game);
    let group = yard_now(&game);
    assert!(has(&group, 22) && !has(&group, 21));
    let whole_data = Blob::of(&game_data(&game, YARD, YARD_SIMGROUP));
    assert_eq!(
        mods.get("whole").unwrap().manifest.resources[0].payload,
        whole_data
    );

    // Below the delta, the whole resource is what the delta is applied to.
    mods.move_mod("merging", 1).unwrap();
    assert!(mods.conflicts(&game, true).unwrap().is_empty());
    apply(&mut mods, &game);
    let group = yard_now(&game);
    assert!(has(&group, 22) && has(&group, 21));
    assert!(object(&group, 22) < object(&group, 21));
    // The original in the plan is the game's, whoever is first.
    let yard_key = key(&game, YARD, YARD_SIMGROUP);
    let plan = mods.plan(&game).unwrap();
    assert_eq!(
        plan[&yard_key].0.original,
        mods.get("whole").unwrap().manifest.resources[0].original
    );
}

#[test]
fn strings_two_mods_add_and_change_are_all_in_the_table() {
    let (dir, game) = town();
    let pristine = snapshot(dir.path());
    let temp = tempfile::tempdir().unwrap();
    let mut mods = ModManager::open(&game).unwrap();
    let build = |id: &str, change: &dyn Fn(&mut StringTable)| {
        let ws = workspace(&game, &temp.path().join(id), LEVEL, STRINGS, |data| {
            edit_table(data, change)
        });
        let out = temp.path().join(format!("{id}.zip"));
        let report = build_from_workspace(&ws, &info(id, "1.0"), None, None, &out).unwrap();
        assert_eq!(report.deltas, 1, "{id}");
        out
    };
    let a = build("a", &|t| {
        t.insert(0x900, &["Added by a".to_owned()]).unwrap();
        set_text(t, 0x100, "Changed by a");
    });
    let b = build("b", &|t| {
        t.insert(0x901, &["Added by b".to_owned()]).unwrap();
        set_text(t, 0x100, "Changed by b");
        set_text(t, 0x101, "Only b");
    });
    mods.install_file(&game, &a, true).unwrap();
    mods.install_file(&game, &b, true).unwrap();
    let conflicts = mods.conflicts(&game, true).unwrap();
    assert_eq!(conflicts.len(), 1);
    assert!(
        conflicts[0].elements[0].starts_with("string 00000100, language 0:"),
        "{:?}",
        conflicts[0].elements
    );
    apply(&mut mods, &game);
    let table = StringTable::parse(&game_data(&game, LEVEL, STRINGS)).unwrap();
    let text = |hash: u32| table.strings[0][table.find(hash).unwrap()].as_str();
    assert_eq!(text(0x900), "Added by a");
    assert_eq!(text(0x901), "Added by b");
    assert_eq!(text(0x100), "Changed by b");
    assert_eq!(text(0x101), "Only b");
    assert_eq!(text(0x102), "The line number 2 of the game");
    mods.unapply(&game, false).unwrap();
    assert_eq!(snapshot(dir.path()), pristine);
}

// ------------------------------------------------- on top of another mod

#[test]
fn an_extension_survives_the_update_of_the_mod_it_is_made_on() {
    let (dir, game) = town();
    let root = dir.path();
    let pristine = snapshot(root);
    let temp = tempfile::tempdir().unwrap();

    // The base mod, version 1: a crate of its own and a moved crate.
    let base = yard_mod(&game, temp.path(), &info("base", "1.0"), |g| {
        add_crate(g, 50);
        g.set_position(object(g, 1), [5.0, 5.0, 5.0]).unwrap();
    });
    // Its next version, made on the game's own data as well: it no longer
    // has its crate, it moves another crate and adds a new one.
    let base_2 = yard_mod(&game, temp.path(), &info("base", "2.0"), |g| {
        g.set_position(object(g, 5), [6.0, 6.0, 6.0]).unwrap();
        add_crate(g, 51);
    });
    let mut mods = ModManager::open(&game).unwrap();
    mods.install_file(&game, &base, true).unwrap();
    apply(&mut mods, &game);

    // The extension, made on the game with the base in it: it moves the
    // base's crate, changes a crate of the game and adds one.
    let game_now = GameInstance::open(root).unwrap();
    let dir = temp.path().join("ws-ext");
    let ws = workspace(&game_now, &dir, YARD, YARD_SIMGROUP, |data| {
        edit_group(data, |g| {
            g.set_position(object(g, 50), [9.0, 9.0, 9.0]).unwrap();
            light(g, 3, 2);
            add_crate(g, 60);
        })
    });
    let yard_key = key(&game, YARD, YARD_SIMGROUP);
    // The workspace keeps the game's own data as the original; the mod is
    // made on what the base gives.
    assert_eq!(
        ws.change(&yard_key).unwrap().original,
        Blob::of(&pristine_yard(&pristine))
    );
    let with_base = game_data(&game_now, YARD, YARD_SIMGROUP);
    let suggested = suggested_dependencies(&game_now, &ws, None, "ext").unwrap();
    assert_eq!(
        suggested.made_on[&yard_key],
        MadeOn {
            blob: Blob::of(&with_base),
            mods: vec!["base".to_owned()],
            data: Some(with_base.clone()),
        }
    );
    let mut ext_info = info("ext", "1.0");
    assert_eq!(
        add_suggested_dependencies(&mut ext_info, &game_now, &ws, None).unwrap(),
        ["base"]
    );
    let ext = temp.path().join("ext.zip");
    let report = build_from_workspace(&ws, &ext_info, None, None, &ext).unwrap();
    assert_eq!(report.deltas, 1);
    let manifest = manifest_of(&ext);
    assert_eq!(manifest.depends["base"], ">=1.0.0");
    assert_eq!(manifest.resources[0].original, Blob::of(&with_base));
    // Three operations: what the author did, nothing of the base.
    let mut archive = ModArchive::open(&ext).unwrap();
    assert_eq!(archive.delta_operations().unwrap()[0].1, 3);

    let report = mods.install_file(&game_now, &ext, true).unwrap();
    assert!(report.issues.is_empty(), "{:?}", report.issues);
    assert!(mods.conflicts(&game_now, true).unwrap().is_empty());
    apply(&mut mods, &game_now);
    // On the base it was made on, the author's bytes.
    assert_eq!(
        Blob::of(&game_data(&game_now, YARD, YARD_SIMGROUP)),
        ws.change(&yard_key).unwrap().payload
    );
    // Building it again while it is installed gives the same mod: its own
    // installed state is not part of what it is made on.
    let mut again_info = info("ext", "1.0");
    add_suggested_dependencies(&mut again_info, &game_now, &ws, None).unwrap();
    assert_eq!(again_info, ext_info);

    // The base is updated.
    let report = mods.install_file(&game_now, &base_2, true).unwrap();
    assert_eq!(report.replaced_version.as_deref(), Some("1.0"));
    // The extension still fits: one of its changes has lost its target,
    // which is said by object and reason, and stops nothing.
    let issues = mods.check(&game_now).unwrap();
    let [Problem::OperationsSkipped { skipped, applied }] = problems(&issues, "ext")[..] else {
        panic!("{issues:?}");
    };
    assert_eq!(*applied, 2);
    assert_eq!(
        skipped[..],
        [format!(
            "object {}, CSystemCommands.CMD_LoadMatrix: the object is not in the group",
            guid(50)
        )]
    );
    assert!(!issues[0].is_fatal() && issues[0].problem.is_notice());
    assert!(
        issues[0].describe().starts_with("ext: "),
        "{}",
        issues[0].describe()
    );
    let report = apply(&mut mods, &game_now);
    assert_eq!(report.warnings.len(), 1, "{:?}", report.warnings);
    assert!(
        report.warnings[0].contains("1 of its 3 change(s) are left out")
            && report.warnings[0].contains(&guid(50).to_string()),
        "{:?}",
        report.warnings
    );
    let group = yard_now(&game_now);
    assert!(!has(&group, 50), "the base took its crate away");
    assert!(has(&group, 51), "the new crate of the base");
    assert!(has(&group, 60), "the crate of the extension");
    assert_eq!(lit(&group, 3), Some(Value::Int(2)));
    let moved = &group.objects[object(&group, 5)];
    assert_eq!(group.transform(moved).unwrap()[3][..3], [6.0, 6.0, 6.0]);
    let kept = &group.objects[object(&group, 1)];
    assert_eq!(group.transform(kept).unwrap()[3][..3], [10.0, 0.0, 1.0]);

    // Without its base the extension cannot be written, as before.
    mods.set_enabled("base", false).unwrap();
    let issues = mods.check(&game_now).unwrap();
    assert!(
        issues
            .iter()
            .any(|i| matches!(i.problem, Problem::DependencyDisabled { .. })),
        "{issues:?}"
    );
    mods.set_enabled("base", true).unwrap();
    mods.unapply(&game_now, false).unwrap();
    assert_eq!(snapshot(root), pristine);
}

#[test]
fn a_whole_resource_made_on_a_mod_with_a_delta_is_checked_against_the_fold() {
    let (dir, game) = town();
    let temp = tempfile::tempdir().unwrap();
    let base = yard_mod(&game, temp.path(), &info("base", "1.0"), |g| {
        add_crate(g, 50)
    });
    let mut mods = ModManager::open(&game).unwrap();
    mods.install_file(&game, &base, true).unwrap();
    apply(&mut mods, &game);
    let game_now = GameInstance::open(dir.path()).unwrap();
    // A change that is no delta, on the game with the base in it.
    let ws = workspace(
        &game_now,
        &temp.path().join("ws"),
        YARD,
        YARD_SIMGROUP,
        |data| edit_group(data, |g| g.objects[1].flags2 ^= 1),
    );
    let mut top = info("top", "1.0");
    add_suggested_dependencies(&mut top, &game_now, &ws, None).unwrap();
    let out = temp.path().join("top.zip");
    let report = build_from_workspace(&ws, &top, None, None, &out).unwrap();
    assert_eq!(report.deltas, 0);
    let with_base = Blob::of(&game_data(&game_now, YARD, YARD_SIMGROUP));
    assert_eq!(manifest_of(&out).resources[0].original, with_base);
    // Its original is what its dependency folds to here: it fits.
    let report = mods.install_file(&game_now, &out, true).unwrap();
    assert!(report.issues.is_empty(), "{:?}", report.issues);
    let conflicts = mods.conflicts(&game_now, true).unwrap();
    assert!(conflicts.len() == 1 && conflicts[0].patch, "{conflicts:?}");
    apply(&mut mods, &game_now);
    assert_eq!(
        Blob::of(&game_data(&game_now, YARD, YARD_SIMGROUP)),
        manifest_of(&out).resources[0].payload
    );
}

#[test]
fn a_delta_without_anything_to_apply_it_to_does_not_fit() {
    let (_dir, game) = town();
    let temp = tempfile::tempdir().unwrap();
    let path = yard_mod(&game, temp.path(), &info("a", "1.0"), |g| add_crate(g, 21));
    // The same mod for a resource the game does not have.
    let mut manifest = manifest_of(&path);
    let mut archive = ModArchive::open(&path).unwrap();
    let document = archive
        .read_payload(&manifest.resources[0].payload)
        .unwrap();
    manifest.resources[0].key.ids = [0x99; 4];
    let lost = temp.path().join("lost.zip");
    super::super::package::write_package(&lost, &manifest, None, |_| Ok(document.clone())).unwrap();
    let mut mods = ModManager::open(&game).unwrap();
    let report = mods.install_file(&game, &lost, true).unwrap();
    assert_eq!(
        problems(&report.issues, "a"),
        [&Problem::ResourceMissing],
        "{:?}",
        report.issues
    );
    assert!(mods.plan(&game).unwrap().is_empty());
    assert!(mods.apply(&game, ModApplyOptions::default()).is_err());
}

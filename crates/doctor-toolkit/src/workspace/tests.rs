use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use super::*;
use crate::container::ustream::{
    FileHeader, Replacement, ResourceHeader, StreamLayout, StreamWriter,
};
use crate::hash::sdbm;
use crate::io::Endian;
use crate::resource::lh2::StringTable;
use crate::resource::tg4::{PARAM_DATA_SIZE, PARAM_FORMAT, ParamValue, Tg4Header};
use crate::toc::{self, Toc};

pub(crate) const LEVEL: &str = "gf2/level";
pub(crate) const TEXT: &str = "gf2/text/abcd";
/// Resource indices in `gf2/level.str`.
pub(crate) const TEX_HEADER: usize = 0;
pub(crate) const TEX_DATA: usize = 1;
pub(crate) const STRINGS: usize = 2;
pub(crate) const MESH: usize = 3;
pub(crate) const SIMGROUP: usize = 4;

pub(crate) fn header(tag: &[u8], ids: [u32; 4]) -> ResourceHeader {
    ResourceHeader {
        mem_policy: sdbm(b"default"),
        alignment: 0x10,
        type_id: sdbm(tag),
        ids,
        data_size: 0,
    }
}

pub(crate) fn sample_table(words: [&str; 2]) -> StringTable {
    StringTable {
        version: 1,
        flags: 0x4000,
        languages: Vec::new(),
        hashes: vec![1, 2],
        strings: vec![words.iter().map(|w| w.to_string()).collect()],
    }
}

/// A table of contents listing `streams` (name, data size) without dependencies.
pub(crate) fn toc_bytes(streams: &[(&str, u64)]) -> Vec<u8> {
    let count = streams.len();
    let names_start = 0x10 + count * 20;
    let pool_start = names_start + count * 8;
    let mut out = Vec::new();
    out.extend_from_slice(&toc::MAGIC.to_le_bytes());
    out.extend_from_slice(&10u32.to_le_bytes());
    for value in [count as u16, count as u16, 0, 0] {
        out.extend_from_slice(&value.to_le_bytes());
    }
    for (i, (_, size)) in streams.iter().enumerate() {
        out.extend_from_slice(&((names_start + i * 8) as u32).to_le_bytes());
        out.extend_from_slice(&(*size as u32).to_le_bytes());
        out.extend_from_slice(&[0u8; 4]);
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
    }
    let mut string_offset = pool_start + 1;
    for (i, (name, _)) in streams.iter().enumerate() {
        out.extend_from_slice(&(0x100 + i as u32).to_le_bytes());
        out.extend_from_slice(&(string_offset as u32).to_le_bytes());
        string_offset += name.len() + 1;
    }
    out.push(0);
    for (name, _) in streams {
        out.extend_from_slice(name.replace('/', "\\").as_bytes());
        out.push(0);
    }
    out.resize(toc::PADDING, 0);
    out
}

/// A miniature game: `gf2/level.str` with a texture, a string table and a
/// mesh, `gf2/text/abcd.str` with a string table, and a table of contents.
pub(crate) fn fake_game() -> (tempfile::TempDir, GameInstance) {
    let mut tex = Tg4Header {
        version: 3,
        budget_hash: 0,
        package: "pkg".into(),
        name_hash: 0x55,
        name: "wall".into(),
        data_size: 8,
        width: 4,
        height: 4,
        padding: 0,
        surface_count: 1,
        params: Vec::new(),
    };
    tex.set_param(PARAM_FORMAT, ParamValue::Str("DXT1c".into()));
    tex.set_param(PARAM_DATA_SIZE, ParamValue::Int(8));

    let mut level = StreamWriter::new(&FileHeader::GF2_PC, Endian::Little);
    level
        .push_resource(
            &header(b"tg4h", [0, sdbm(b"tg4h"), 7, 0x55]),
            &tex.to_bytes().unwrap(),
            Storage::Raw,
        )
        .unwrap();
    level
        .push_resource(
            &header(b"tg4d", [0, sdbm(b"tg4d"), 7, 0x55]),
            &[0u8; 8],
            Storage::RefPack,
        )
        .unwrap();
    level
        .push_resource(
            &header(b"lh2", [9; 4]),
            &sample_table(["Hello", "World"]).to_bytes().unwrap(),
            Storage::Raw,
        )
        .unwrap();
    level
        .push_resource(&header(b"mesh", [3; 4]), &[7u8; 300], Storage::RefPack)
        .unwrap();
    level
        .push_resource(
            &header(b"simgroup", [4; 4]),
            &crate::resource::simgroup::tests::sample(),
            Storage::RefPack,
        )
        .unwrap();
    let level = level.finish();

    let mut text = StreamWriter::new(&FileHeader::GF2_PC, Endian::Little);
    text.push_resource(
        &header(b"lh2", [2; 4]),
        &sample_table(["Yes", "No"]).to_bytes().unwrap(),
        Storage::Raw,
    )
    .unwrap();
    let text = text.finish();

    let size = |data: &[u8]| StreamLayout::parse(data).unwrap().total_data_size();
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    fs::create_dir_all(root.join("gf2/text")).unwrap();
    fs::write(root.join("godfather2.exe"), b"MZ fake").unwrap();
    fs::write(root.join("gf2/level.str"), &level).unwrap();
    fs::write(root.join("gf2/text/abcd.str"), &text).unwrap();
    let table = Toc::parse(&toc_bytes(&[(LEVEL, size(&level)), (TEXT, size(&text))])).unwrap();
    fs::write(root.join("gf2/level.toc"), table.to_bytes().unwrap()).unwrap();
    let game = GameInstance::open(root).unwrap();
    (dir, game)
}

pub(crate) fn stream(game: &GameInstance, name: &str) -> usize {
    game.find_stream(name).unwrap()
}

pub(crate) fn key(game: &GameInstance, name: &str, index: usize) -> ResourceKey {
    let file = game.open_stream(stream(game, name)).unwrap();
    ResourceKey::new(name, &file.layout().resources[index].header)
}

pub(crate) fn game_data(game: &GameInstance, name: &str, index: usize) -> Vec<u8> {
    game.open_stream(stream(game, name))
        .unwrap()
        .resource_data(index)
        .unwrap()
}

pub(crate) fn toc_size(root: &Path, name: &str) -> u32 {
    let table = Toc::parse(&fs::read(root.join("gf2/level.toc")).unwrap()).unwrap();
    table.files[table.find_file(name).unwrap()].data_size
}

pub(crate) fn edit(index: usize, data: Vec<u8>) -> ResourceEdit {
    ResourceEdit {
        index,
        data,
        storage: StorageChoice::Keep,
    }
}

/// Every file under `root` except the tool directory, with its contents.
pub(crate) fn snapshot(root: &Path) -> BTreeMap<String, Vec<u8>> {
    fn walk(dir: &Path, rel: &str, out: &mut BTreeMap<String, Vec<u8>>) {
        for entry in fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            let name = entry.file_name().into_string().unwrap();
            let rel = if rel.is_empty() {
                name.clone()
            } else {
                format!("{rel}/{name}")
            };
            if name == "_doctor_toolkit" {
                continue;
            }
            if entry.path().is_dir() {
                walk(&entry.path(), &rel, out);
            } else {
                out.insert(rel, fs::read(entry.path()).unwrap());
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(root, "", &mut out);
    out
}

#[test]
fn records_without_touching_the_game_then_applies_and_unapplies() {
    let (dir, game) = fake_game();
    let root = dir.path();
    let before = snapshot(root);
    let mut ws = Workspace::for_game(&game, None).unwrap();
    assert_eq!(ws.dir(), root.join(DEFAULT_DIR));

    let table = sample_table(["Hello", "Mundo!"]);
    assert!(
        ws.set_string_table(&game, stream(&game, LEVEL), STRINGS, &table)
            .unwrap()
    );
    let strings = key(&game, LEVEL, STRINGS);
    assert_eq!(snapshot(root), before, "recording leaves the game alone");
    assert_eq!(
        ws.data(&strings).unwrap().unwrap(),
        table.to_bytes().unwrap()
    );
    let status = ws.status(&game).unwrap();
    assert!(status.fingerprint_matches);
    assert_eq!(status.changes, [(strings.clone(), ChangeState::Pending)]);
    assert!(!status.is_up_to_date());

    // The workspace survives a restart.
    let reopened = Workspace::open(ws.dir()).unwrap();
    assert_eq!(reopened.changes().count(), 1);
    assert_eq!(reopened.undo_label(), Some("Edit strings in gf2/level"));

    let report = ws.apply(&game, ApplyOptions::default()).unwrap();
    assert_eq!(
        report.streams,
        [(LEVEL.to_owned(), StreamAction::Rebuilt(1))]
    );
    assert_eq!(report.written, ["gf2/level.str", "gf2/level.toc"]);
    assert_eq!(report.backed_up, report.written);
    assert!(report.warnings.is_empty());
    assert_eq!(
        StringTable::parse(&game_data(&game, LEVEL, STRINGS)).unwrap(),
        table
    );
    assert_eq!(game_data(&game, LEVEL, MESH), vec![7u8; 300]);
    let level = fs::read(root.join("gf2/level.str")).unwrap();
    assert_eq!(
        toc_size(root, LEVEL) as u64,
        StreamLayout::parse(&level).unwrap().total_data_size()
    );
    let status = ws.status(&game).unwrap();
    assert_eq!(status.changes, [(strings.clone(), ChangeState::Applied)]);
    assert!(status.is_up_to_date());
    // A second apply has nothing to do.
    assert_eq!(
        ws.apply(&game, ApplyOptions::default()).unwrap(),
        ApplyReport::default()
    );

    let report = ws.unapply(&game, false).unwrap();
    assert_eq!(report.streams, [(LEVEL.to_owned(), StreamAction::Restored)]);
    assert_eq!(
        snapshot(root),
        before,
        "unapply restores the original bytes"
    );
    let status = ws.status(&game).unwrap();
    assert_eq!(status.changes, [(strings, ChangeState::Pending)]);
}

#[test]
fn texture_import_is_one_undoable_operation() {
    use crate::resource::pixels;
    let (dir, game) = fake_game();
    let mut ws = Workspace::for_game(&game, Some(&dir.path().join("elsewhere"))).unwrap();
    let pair = TexturePair {
        header: TEX_HEADER,
        data: TEX_DATA,
    };
    let rgba: Vec<u8> = (0..8 * 8).flat_map(|i| [i as u8, 0, 0, 255]).collect();
    let png = pixels::encode_png(8, 8, &rgba).unwrap();
    let texture = ws
        .import_texture(&game, stream(&game, LEVEL), pair, &png, true)
        .unwrap();
    assert_eq!((texture.header.width, texture.header.height), (8, 8));
    assert_eq!(ws.len(), 2);
    assert_eq!(ws.operations().len(), 1);
    let tex_header = key(&game, LEVEL, TEX_HEADER);
    let detail = ws.detail(&tex_header).unwrap().unwrap();
    let Some(SemanticDiff::Texture(shape)) = detail.semantic else {
        panic!("texture diff expected");
    };
    assert_eq!((shape.old.width, shape.new.width), (4, 8));
    assert!(!detail.bytes.is_identical());

    assert_eq!(ws.undo().unwrap().as_deref(), Some("Import texture wall"));
    assert!(ws.is_empty());
    assert_eq!(ws.undo().unwrap(), None);
    assert_eq!(ws.redo().unwrap().as_deref(), Some("Import texture wall"));
    assert_eq!(ws.len(), 2);
    assert_eq!(ws.redo().unwrap(), None);

    // Direct mode: record and apply this stream at once.
    ws.apply(
        &game,
        ApplyOptions {
            streams: Some(vec![LEVEL.into()]),
            ..Default::default()
        },
    )
    .unwrap();
    let file = game.open_stream(stream(&game, LEVEL)).unwrap();
    let loaded = Texture::load(file.data(), file.layout(), pair).unwrap();
    assert_eq!(loaded.data, texture.data);
    assert!(file.layout().is_compressed(TEX_DATA), "storage is kept");
}

#[test]
fn simgroup_edits_build_on_each_other_and_show_in_the_diff() {
    use crate::resource::simgroup::{SimGroup, Value};
    let (_dir, game) = fake_game();
    let mut ws = Workspace::for_game(&game, None).unwrap();
    let level = stream(&game, LEVEL);
    let edit = |ws: &Workspace, value: i32| {
        let data = ws.current_data(&game, level, SIMGROUP).unwrap();
        let mut group = SimGroup::parse(&data).unwrap();
        group.set_attr(0, 0, 1, Some(Value::Int(value))).unwrap();
        group.encode()
    };
    let first = edit(&ws, 5);
    assert!(
        ws.set_resource(&game, level, SIMGROUP, first, "Set 5")
            .unwrap()
    );
    let second = edit(&ws, 6);
    assert!(
        ws.set_resource(&game, level, SIMGROUP, second.clone(), "Set 6")
            .unwrap()
    );
    assert_eq!(ws.current_data(&game, level, SIMGROUP).unwrap(), second);
    let detail = ws.detail(&key(&game, LEVEL, SIMGROUP)).unwrap().unwrap();
    let Some(SemanticDiff::Logic(diff)) = detail.semantic else {
        panic!("logic diff expected");
    };
    assert_eq!(diff.changed, [0]);
    assert_eq!(diff.values.len(), 1);
    assert_eq!(
        (diff.values[0].old.as_str(), diff.values[0].new.as_str()),
        ("default", "00000006")
    );
    assert_eq!(ws.undo().unwrap().as_deref(), Some("Set 6"));
    assert_eq!(ws.undo().unwrap().as_deref(), Some("Set 5"));
    assert!(ws.is_empty());
    assert_eq!(
        ws.current_data(&game, level, SIMGROUP).unwrap(),
        game_data(&game, LEVEL, SIMGROUP)
    );
}

#[test]
fn setting_the_original_data_removes_the_change() {
    let (_dir, game) = fake_game();
    let mut ws = Workspace::for_game(&game, None).unwrap();
    let level = stream(&game, LEVEL);
    let original = game_data(&game, LEVEL, MESH);
    let mesh = key(&game, LEVEL, MESH);
    ws.set_resources(&game, "mesh", level, &[edit(MESH, vec![1; 10])])
        .unwrap();
    assert_eq!(ws.change(&mesh).unwrap().original, Blob::of(&original));
    // The same data again is no operation.
    assert_eq!(
        ws.set_resources(&game, "same", level, &[edit(MESH, vec![1; 10])])
            .unwrap(),
        None
    );
    ws.set_resources(&game, "back", level, &[edit(MESH, original)])
        .unwrap();
    assert!(ws.is_empty());
    assert_eq!(ws.operations().len(), 2);
    // Reverting nothing records nothing.
    assert!(!ws.revert(std::slice::from_ref(&mesh), "revert").unwrap());
    ws.undo().unwrap();
    assert!(ws.revert(&[mesh], "revert").unwrap());
    assert!(ws.is_empty());
    assert_eq!(ws.operations().len(), 2, "the undone operation is dropped");
    assert!(
        ws.set_resources(&game, "bad", level, &[edit(99, vec![])])
            .is_err()
    );
}

#[test]
fn diff_lists_changes_with_string_details() {
    let (_dir, game) = fake_game();
    let mut ws = Workspace::for_game(&game, None).unwrap();
    ws.set_string_table(
        &game,
        stream(&game, TEXT),
        0,
        &sample_table(["Yes", "Nope"]),
    )
    .unwrap();
    ws.set_resources(
        &game,
        "mesh",
        stream(&game, LEVEL),
        &[edit(MESH, vec![7; 310])],
    )
    .unwrap();
    let diff = ws.diff();
    assert_eq!(diff.len(), 2);
    assert_eq!(diff[0].key.stream, LEVEL);
    assert_eq!((diff[0].original_size, diff[0].new_size), (300, 310));
    let detail = ws.detail(&diff[0].key).unwrap().unwrap();
    assert_eq!(detail.bytes.ranges.len(), 1);
    assert_eq!(detail.bytes.ranges[0], 300..310);
    assert!(detail.semantic.is_none());
    let Some(SemanticDiff::Strings(strings)) = ws.detail(&diff[1].key).unwrap().unwrap().semantic
    else {
        panic!("string diff expected");
    };
    assert_eq!(strings.changed.len(), 1);
    assert_eq!(
        (&*strings.changed[0].old, &*strings.changed[0].new),
        ("No", "Nope")
    );
}

#[test]
fn external_changes_are_detected_and_need_force() {
    let (dir, game) = fake_game();
    let root = dir.path();
    let mut ws = Workspace::for_game(&game, None).unwrap();
    let level = stream(&game, LEVEL);
    ws.set_resources(&game, "mesh", level, &[edit(MESH, vec![1; 10])])
        .unwrap();
    ws.apply(&game, ApplyOptions::default()).unwrap();

    // Another tool rewrites the stream (different size).
    game.replace_resources(
        level,
        &[Replacement {
            index: MESH,
            data: &[2; 20],
            storage: Storage::Raw,
        }],
        Default::default(),
    )
    .unwrap();
    let status = ws.status(&game).unwrap();
    assert_eq!(status.conflicts(), [LEVEL]);
    assert_eq!(status.count(ChangeState::Conflict), 1);
    let error = ws.apply(&game, ApplyOptions::default()).unwrap_err();
    assert!(error.to_string().contains("changed by another program"));
    assert_eq!(game_data(&game, LEVEL, MESH), vec![2; 20]);

    ws.apply(
        &game,
        ApplyOptions {
            force: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(game_data(&game, LEVEL, MESH), vec![1; 10]);
    assert!(ws.status(&game).unwrap().is_up_to_date());

    // `restore` puts the originals back behind the workspace's back: the
    // changes become pending again, not conflicts.
    game.restore_backups().unwrap();
    let status = ws.status(&game).unwrap();
    assert_eq!(status.count(ChangeState::Pending), 1);
    assert_eq!(status.streams[0].state, StreamState::Original);
    ws.apply(&game, ApplyOptions::default()).unwrap();
    assert_eq!(game_data(&game, LEVEL, MESH), vec![1; 10]);
    assert!(
        root.join(crate::game::BACKUP_DIR)
            .join("gf2/level.str")
            .is_file()
    );
}

#[test]
fn a_direct_write_outside_the_workspace_is_a_conflict() {
    let (_dir, game) = fake_game();
    let level = stream(&game, LEVEL);
    game.replace_resources(
        level,
        &[Replacement {
            index: MESH,
            data: &[5; 5],
            storage: Storage::Raw,
        }],
        Default::default(),
    )
    .unwrap();
    let mut ws = Workspace::for_game(&game, None).unwrap();
    ws.set_string_table(&game, level, STRINGS, &sample_table(["A", "B"]))
        .unwrap();
    // The original is taken from the backup, not from the modified file.
    let mesh_original = {
        let mut ws2 = Workspace::create(ws.dir().with_file_name("ws2"), &game).unwrap();
        ws2.set_resources(&game, "m", level, &[edit(MESH, vec![0])])
            .unwrap();
        ws2.changes().next().unwrap().original.size
    };
    assert_eq!(mesh_original, 300);
    assert_eq!(ws.status(&game).unwrap().conflicts(), [LEVEL]);
    assert!(ws.apply(&game, ApplyOptions::default()).is_err());
}

#[test]
fn reapplying_after_revert_restores_the_original_stream() {
    let (dir, game) = fake_game();
    let before = snapshot(dir.path());
    let mut ws = Workspace::for_game(&game, None).unwrap();
    ws.set_resources(
        &game,
        "two streams",
        stream(&game, LEVEL),
        &[edit(MESH, vec![1; 1000])],
    )
    .unwrap();
    ws.set_string_table(&game, stream(&game, TEXT), 0, &sample_table(["Si", "No"]))
        .unwrap();
    ws.apply(&game, ApplyOptions::default()).unwrap();
    assert_ne!(snapshot(dir.path()), before);

    let text_key = key(&game, TEXT, 0);
    ws.revert(std::slice::from_ref(&text_key), "revert text")
        .unwrap();
    let status = ws.status(&game).unwrap();
    assert_eq!(status.pending_reverts, [text_key]);
    let report = ws.apply(&game, ApplyOptions::default()).unwrap();
    assert_eq!(report.streams, [(TEXT.to_owned(), StreamAction::Restored)]);
    assert_eq!(
        fs::read(dir.path().join("gf2/text/abcd.str")).unwrap(),
        before["gf2/text/abcd.str"]
    );
    ws.revert_all().unwrap();
    ws.apply(&game, ApplyOptions::default()).unwrap();
    assert_eq!(snapshot(dir.path()), before);
    assert!(ws.status(&game).unwrap().streams.is_empty());
}

#[test]
fn interrupted_saves_are_repaired_on_open() {
    let (_dir, game) = fake_game();
    let mut ws = Workspace::for_game(&game, None).unwrap();
    let level = stream(&game, LEVEL);
    ws.set_resources(&game, "one", level, &[edit(MESH, vec![1])])
        .unwrap();
    let manifest_after_one = fs::read(ws.dir().join(MANIFEST_FILE)).unwrap();
    ws.set_resources(&game, "two", level, &[edit(MESH, vec![2])])
        .unwrap();
    let mesh = key(&game, LEVEL, MESH);

    // The log was written, the manifest was not.
    fs::write(ws.dir().join(MANIFEST_FILE), &manifest_after_one).unwrap();
    let reopened = Workspace::open(ws.dir()).unwrap();
    assert_eq!(reopened.data(&mesh).unwrap().unwrap(), [2]);
    assert_eq!(reopened.head(), 2);

    // After an undo the log is behind the manifest's state.
    let mut ws = reopened;
    let manifest_before_undo = fs::read(ws.dir().join(MANIFEST_FILE)).unwrap();
    ws.undo().unwrap();
    fs::write(ws.dir().join(MANIFEST_FILE), &manifest_before_undo).unwrap();
    let reopened = Workspace::open(ws.dir()).unwrap();
    assert_eq!(reopened.data(&mesh).unwrap().unwrap(), [1]);
    assert_eq!(reopened.redo_label(), Some("two"));
}

#[test]
fn a_second_writer_is_refused() {
    let (_dir, game) = fake_game();
    let mut first = Workspace::for_game(&game, None).unwrap();
    let mut second = Workspace::for_game(&game, None).unwrap();
    let level = stream(&game, LEVEL);
    first
        .set_resources(&game, "a", level, &[edit(MESH, vec![1])])
        .unwrap();
    let error = second
        .set_resources(&game, "b", level, &[edit(MESH, vec![2])])
        .unwrap_err();
    assert!(error.to_string().contains("another program"));
    second.reload().unwrap();
    second
        .set_resources(&game, "b", level, &[edit(MESH, vec![2])])
        .unwrap();
}

#[test]
fn an_interrupted_apply_is_recognised() {
    let (dir, game) = fake_game();
    let mut ws = Workspace::for_game(&game, None).unwrap();
    let level = stream(&game, LEVEL);
    ws.set_resources(&game, "a", level, &[edit(MESH, vec![1; 3])])
        .unwrap();
    ws.apply(&game, ApplyOptions::default()).unwrap();
    // Pretend the program died after the rename but before the final record.
    let path = ws.dir().join(APPLIED_FILE);
    let mut applied: Deployment = read_json(&path).unwrap();
    let record = applied.streams.get_mut(LEVEL).unwrap();
    record.in_flight = Some(FileStamp {
        mtime_ns: None,
        file_id: None,
        ..record.written.clone()
    });
    record.written = FileStamp {
        sha256: "0".repeat(64),
        ..record.original.clone()
    };
    write_json(&path, &applied).unwrap();
    let ws = Workspace::open(ws.dir()).unwrap();
    assert_eq!(
        ws.status(&game).unwrap().streams[0].state,
        StreamState::Applied
    );
    // The original file must not have been touched by the failed record.
    assert!(dir.path().join("gf2/level.str").is_file());
}

#[test]
fn manifest_format_is_stable() {
    let (_dir, game) = fake_game();
    let mut ws = Workspace::for_game(&game, None).unwrap();
    ws.set_resources(
        &game,
        "mesh",
        stream(&game, LEVEL),
        &[ResourceEdit {
            index: MESH,
            data: b"new".to_vec(),
            storage: StorageChoice::Raw,
        }],
    )
    .unwrap();
    let manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(ws.dir().join(MANIFEST_FILE)).unwrap()).unwrap();
    assert_eq!(manifest["format"], FORMAT);
    assert_eq!(manifest["version"], VERSION);
    assert_eq!(manifest["game"]["profile"], "The Godfather II (PC)");
    assert_eq!(manifest["game"]["executable_size"], 7);
    let change = &manifest["resources"][0];
    assert_eq!(change["key"], "gf2/level:mesh:00000003");
    assert_eq!(change["payload"]["size"], 3);
    assert_eq!(change["original"]["size"], 300);
    assert_eq!(change["storage"], "raw");
    assert_eq!(change["label"], "mesh");
    assert!(
        ws.store()
            .path(change["payload"]["sha256"].as_str().unwrap())
            .is_file()
    );
}

#[cfg(unix)]
#[test]
fn a_shadow_game_of_symlinks_leaves_the_real_files_alone() {
    let (real_dir, _) = fake_game();
    let real = real_dir.path();
    let before = snapshot(real);
    let shadow_dir = tempfile::tempdir().unwrap();
    let shadow = shadow_dir.path();
    for rel in before.keys() {
        let link = shadow.join(rel);
        fs::create_dir_all(link.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(real.join(rel), link).unwrap();
    }
    let game = GameInstance::open(shadow).unwrap();
    let mut ws = Workspace::for_game(&game, None).unwrap();
    ws.set_resources(
        &game,
        "mesh",
        stream(&game, LEVEL),
        &[edit(MESH, vec![1; 2000])],
    )
    .unwrap();
    ws.apply(&game, ApplyOptions::default()).unwrap();
    assert_eq!(game_data(&game, LEVEL, MESH), vec![1; 2000]);
    assert_eq!(snapshot(real), before, "the real game is untouched");
    ws.unapply(&game, false).unwrap();
    assert_eq!(snapshot(shadow), before);
    game.restore_backups().unwrap();
    assert_eq!(snapshot(real), before);
}

mod undo_redo {
    use super::*;
    use proptest::prelude::*;

    #[derive(Debug, Clone)]
    enum Step {
        Set(usize, u8),
        Revert(usize),
        Undo,
        Redo,
    }

    fn step() -> impl Strategy<Value = Step> {
        prop_oneof![
            3 => (0..3usize, 0..4u8).prop_map(|(r, v)| Step::Set(r, v)),
            1 => (0..3usize).prop_map(Step::Revert),
            2 => Just(Step::Undo),
            2 => Just(Step::Redo),
        ]
    }

    /// The resources the steps edit: (stream, index).
    const TARGETS: [(&str, usize); 3] = [(LEVEL, MESH), (LEVEL, STRINGS), (TEXT, 0)];

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(24))]
        #[test]
        fn matches_a_model_of_states(steps in proptest::collection::vec(step(), 1..24)) {
            let (_dir, game) = fake_game();
            let mut ws = Workspace::for_game(&game, None).unwrap();
            let keys: Vec<_> = TARGETS.iter().map(|(s, i)| key(&game, s, *i)).collect();
            let originals: Vec<_> = TARGETS.iter().map(|(s, i)| game_data(&game, s, *i)).collect();
            // Value 0 means "the original data".
            let data = |target: usize, value: u8| {
                if value == 0 { originals[target].clone() } else { vec![value; value as usize] }
            };
            let mut history: Vec<BTreeMap<usize, Vec<u8>>> = vec![BTreeMap::new()];
            let mut head = 0;
            for step in steps {
                let current = history[head].clone();
                let next = match &step {
                    Step::Set(t, v) => {
                        let (name, index) = TARGETS[*t];
                        ws.set_resources(&game, "set", stream(&game, name), &[edit(index, data(*t, *v))]).unwrap();
                        let mut next = current.clone();
                        if *v == 0 { next.remove(t); } else { next.insert(*t, data(*t, *v)); }
                        Some(next)
                    }
                    Step::Revert(t) => {
                        ws.revert(&[keys[*t].clone()], "revert").unwrap();
                        let mut next = current.clone();
                        next.remove(t);
                        Some(next)
                    }
                    Step::Undo => {
                        ws.undo().unwrap();
                        head = head.saturating_sub(1);
                        None
                    }
                    Step::Redo => {
                        ws.redo().unwrap();
                        if head + 1 < history.len() { head += 1; }
                        None
                    }
                };
                if let Some(next) = next && next != current {
                    history.truncate(head + 1);
                    history.push(next);
                    head += 1;
                }
                let expected = &history[head];
                prop_assert_eq!(ws.len(), expected.len());
                for (t, bytes) in expected {
                    let actual = ws.data(&keys[*t]).unwrap();
                    prop_assert_eq!(actual.as_ref(), Some(bytes));
                }
                prop_assert_eq!(ws.head(), head);
                prop_assert_eq!(ws.operations().len(), history.len() - 1);
            }
            if !ws.is_saved() {
                // Nothing was ever recorded: no workspace on disk.
                prop_assert!(ws.operations().is_empty());
                return Ok(());
            }
            let reopened = Workspace::open(ws.dir()).unwrap();
            prop_assert_eq!(reopened.changes().collect::<Vec<_>>(), ws.changes().collect::<Vec<_>>());
            prop_assert_eq!(reopened.head(), ws.head());
            // Every payload still referenced is present.
            for change in reopened.changes() {
                prop_assert!(reopened.store().contains(&change.payload));
            }
        }
    }
}

// ------------------------------------------------- added resources and streams

pub(crate) fn new_resource(stream: &str, tag: &[u8], id: u32, data: &[u8]) -> NewResourceEdit {
    let mut header = header(tag, [id; 4]);
    let defaults = crate::create::type_defaults(header.type_id);
    header.mem_policy = defaults.mem_policy;
    header.alignment = defaults.alignment;
    NewResourceEdit {
        stream: stream.into(),
        header,
        data: data.to_vec(),
        storage: StorageChoice::Keep,
        before: None,
    }
}

pub(crate) fn new_stream(name: &str, with_parent: bool) -> NewStream {
    NewStream {
        name: name.into(),
        guid: "00".repeat(16),
        hash: 0x0777_0000,
        parent: LEVEL.into(),
        with_parent,
        auto_load: false,
        tocs: vec!["gf2/level.toc".into()],
        toc_parents: Default::default(),
        label: String::new(),
    }
}

#[test]
fn added_resources_are_inserted_and_removed_with_undo() {
    let (dir, game) = fake_game();
    let root = dir.path();
    let before = snapshot(root);
    let mut ws = Workspace::for_game(&game, None).unwrap();

    let mesh_key = key(&game, LEVEL, MESH);
    let mut first = new_resource(LEVEL, b"metamodel", 0x1234, b"placed before the mesh");
    first.before = Some(mesh_key.clone());
    let second = new_resource(LEVEL, b"lua", 0x5678, &[1u8; 70_000]);
    ws.add_resources(&game, "Add two", &[first, second])
        .unwrap();
    // A GUID already in the stream or in the workspace is refused.
    let duplicate = new_resource(LEVEL, b"mesh", 3, b"x");
    assert!(ws.add_resources(&game, "dup", &[duplicate]).is_err());
    let again = new_resource(LEVEL, b"lua", 0x5678, b"x");
    assert!(ws.add_resources(&game, "dup", &[again]).is_err());
    assert_eq!(snapshot(root), before, "recording leaves the game alone");
    assert!(ws.diff().iter().all(|d| d.added && d.original_size == 0));

    let status = ws.status(&game).unwrap();
    assert_eq!(status.count(ChangeState::Pending), 2);
    ws.apply(&game, ApplyOptions::default()).unwrap();
    let status = ws.status(&game).unwrap();
    assert_eq!(status.count(ChangeState::Applied), 2);
    assert!(status.is_up_to_date());

    let file = game.open_stream(stream(&game, LEVEL)).unwrap();
    let layout = file.layout();
    let types: Vec<u32> = layout.resources.iter().map(|r| r.header.type_id).collect();
    // The original resources, with the metamodel before the mesh and the lua at the end.
    let original = StreamLayout::parse(&before["gf2/level.str"]).unwrap();
    let mut expected: Vec<u32> = original
        .resources
        .iter()
        .map(|r| r.header.type_id)
        .collect();
    expected.insert(MESH, sdbm(b"metamodel"));
    expected.push(sdbm(b"lua"));
    assert_eq!(types, expected);
    let last = layout.resources.len() - 1;
    assert_eq!(file.resource_data(MESH).unwrap(), b"placed before the mesh");
    assert_eq!(file.resource_data(last).unwrap(), vec![1u8; 70_000]);
    assert_eq!(layout.resources[last].header.ids, [0x5678; 4]);
    assert_eq!(layout.resources[MESH].header.mem_policy, sdbm(b"default"));
    assert_eq!(toc_size(root, LEVEL) as u64, layout.total_data_size());
    drop(file);

    // Undo removes them from the game on the next apply; the files return to the originals.
    ws.undo().unwrap();
    ws.apply(&game, ApplyOptions::default()).unwrap();
    assert_eq!(snapshot(root), before);
    ws.redo().unwrap();
    ws.apply(&game, ApplyOptions::default()).unwrap();
    let count = game
        .open_stream(stream(&game, LEVEL))
        .unwrap()
        .layout()
        .resources
        .len();
    assert_eq!(count, original.resources.len() + 2);
    ws.unapply(&game, false).unwrap();
    assert_eq!(snapshot(root), before);
}

#[test]
fn new_streams_are_created_listed_and_deleted() {
    for (with_parent, auto_load) in [(false, false), (true, false), (true, true)] {
        let (dir, game) = fake_game();
        let root = dir.path();
        let before = snapshot(root);
        let mut ws = Workspace::for_game(&game, None).unwrap();
        let name = "gf2/doctor/props";
        let mut created = new_stream(name, with_parent);
        created.auto_load = auto_load;
        ws.add_stream(&game, created).unwrap();
        // The same name, a name of the game or a used hash are refused.
        assert!(ws.add_stream(&game, new_stream(name, false)).is_err());
        assert!(ws.add_stream(&game, new_stream(TEXT, false)).is_err());
        let mut taken = new_stream("gf2/doctor/other", false);
        taken.hash = 0x100;
        assert!(ws.add_stream(&game, taken).is_err());

        let table_bytes = sample_table(["New", "Stream"]).to_bytes().unwrap();
        ws.add_resources(
            &game,
            "Add to the new stream",
            &[
                new_resource(name, b"lh2", 0xAA, &table_bytes),
                new_resource(name, b"mesh", 0xBB, &[5u8; 1000]),
            ],
        )
        .unwrap();
        assert_eq!(snapshot(root), before);
        let reopened = Workspace::open(ws.dir()).unwrap();
        assert_eq!(reopened.new_streams().count(), 1);

        ws.apply(&game, ApplyOptions::default()).unwrap();
        let path = root.join("gf2/doctor/props.str");
        let data = fs::read(&path).unwrap();
        let layout = StreamLayout::parse(&data).unwrap();
        assert_eq!(layout.resources.len(), 2);
        assert_eq!(layout.resources[0].header.ids, [0xAA; 4]);
        assert_eq!(layout.resources[1].header.mem_policy, sdbm(b"geometry"));
        assert!(ws.status(&game).unwrap().is_up_to_date());

        let table = Toc::parse(&fs::read(root.join("gf2/level.toc")).unwrap()).unwrap();
        assert!(table.is_sorted_by_hash());
        let entry = table.find_file(name).unwrap();
        assert_eq!(table.file_name(entry).hash, 0x0777_0000);
        assert_eq!(table.file_name(entry).name_lossy(), "gf2\\doctor\\props");
        assert_eq!(
            table.files[entry].data_size as u64,
            layout.total_data_size()
        );
        let deps: Vec<String> = table.file_deps(entry).map(|n| n.name_lossy()).collect();
        assert_eq!(deps, ["gf2\\level"]);
        let level = table.find_file(LEVEL).unwrap();
        let children: Vec<String> = table.file_deps(level).map(|n| n.name_lossy()).collect();
        if with_parent && auto_load {
            // A child that is not disabled by default: the game opens it
            // with every load of the parent.
            assert_eq!(table.files[entry].flags, toc::FLAGS_CHILD);
            assert_eq!(
                table.files[entry].flags[2] & toc::FLAG_LOAD_DISABLED_DEFAULT,
                0
            );
            assert_eq!(children, ["gf2\\doctor\\props"]);
        } else if with_parent {
            assert_eq!(table.files[entry].flags, toc::FLAGS_PARENTED);
            assert_eq!(children, ["gf2\\doctor\\props"]);
        } else {
            assert_eq!(table.files[entry].flags, toc::FLAGS_REQUESTED);
            assert!(children.is_empty());
        }

        // Editing another stream keeps the new entry.
        let edited = sample_table(["Hello", "Changed"]);
        ws.set_string_table(&game, stream(&game, LEVEL), STRINGS, &edited)
            .unwrap();
        ws.apply(&game, ApplyOptions::default()).unwrap();
        let table = Toc::parse(&fs::read(root.join("gf2/level.toc")).unwrap()).unwrap();
        assert!(table.find_file(name).is_some());
        assert_ne!(snapshot(root)["gf2/level.str"], before["gf2/level.str"]);

        // Undo the edit and the stream: the file is deleted, the table is the original.
        ws.undo().unwrap();
        ws.undo().unwrap();
        ws.undo().unwrap();
        assert_eq!(ws.new_streams().count(), 0);
        ws.apply(&game, ApplyOptions::default()).unwrap();
        assert!(!path.exists());
        assert_eq!(snapshot(root), before);

        // Redo and unapply: same result.
        ws.redo().unwrap();
        ws.redo().unwrap();
        ws.apply(&game, ApplyOptions::default()).unwrap();
        assert!(path.exists());
        ws.unapply(&game, false).unwrap();
        assert!(!path.exists());
        assert_eq!(snapshot(root), before);
        assert_eq!(ws.status(&game).unwrap().count(ChangeState::Pending), 2);

        // Removing the stream removes its resources too.
        assert!(ws.remove_stream(name).unwrap());
        assert!(ws.is_empty());
    }
}

#[test]
fn version_1_workspaces_open_and_are_saved_as_version_2() {
    let (_dir, game) = fake_game();
    let mut ws = Workspace::for_game(&game, None).unwrap();
    ws.set_resources(
        &game,
        "mesh",
        stream(&game, LEVEL),
        &[edit(MESH, b"new".to_vec())],
    )
    .unwrap();
    // Rewrite the manifest as version 1 wrote it.
    let path = ws.dir().join(MANIFEST_FILE);
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    manifest["version"] = 1.into();
    manifest.as_object_mut().unwrap().remove("streams");
    fs::write(&path, serde_json::to_vec_pretty(&manifest).unwrap()).unwrap();
    assert!(!fs::read_to_string(&path).unwrap().contains("added"));

    let mut ws = Workspace::open(ws.dir()).unwrap();
    assert_eq!(ws.len(), 1);
    assert!(!ws.changes().next().unwrap().is_added());
    ws.add_stream(&game, new_stream("gf2/doctor/x", false))
        .unwrap();
    let saved: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(saved["version"], 2);
    assert_eq!(saved["streams"][0]["name"], "gf2/doctor/x");
    ws.undo().unwrap();
    ws.undo().unwrap();
    assert!(ws.is_empty());
}

#[test]
fn a_workspace_is_written_by_its_first_operation() {
    let (dir, game) = fake_game();
    let root = dir.path();
    let before = snapshot(root);
    let mut ws = Workspace::for_game(&game, None).unwrap();
    assert!(!ws.is_saved());
    let level = stream(&game, LEVEL);
    // A failed edit, an edit that changes nothing, undo, apply and revert
    // leave no workspace behind.
    assert!(
        ws.set_resources(&game, "bad", level, &[edit(9999, vec![1])])
            .is_err()
    );
    let original = game_data(&game, LEVEL, MESH);
    assert!(
        ws.set_resources(&game, "same", level, &[edit(MESH, original.clone())])
            .unwrap()
            .is_none()
    );
    assert_eq!(ws.undo().unwrap(), None);
    assert!(
        ws.apply(&game, ApplyOptions::default())
            .unwrap()
            .written
            .is_empty()
    );
    assert!(ws.unapply(&game, false).unwrap().written.is_empty());
    ws.reload().unwrap();
    assert!(!Workspace::exists(ws.dir()));
    assert_eq!(snapshot(root), before, "nothing was written");

    ws.set_resources(&game, "edit", level, &[edit(MESH, vec![7; 32])])
        .unwrap();
    assert!(ws.is_saved());
    let reopened = Workspace::open(ws.dir()).unwrap();
    let change = reopened.changes().next().unwrap().clone();
    assert_eq!(reopened.store().get(&change.payload).unwrap(), vec![7; 32]);
    assert_eq!(reopened.store().get(&change.original).unwrap(), original);
}

/// The fake game with two `.viv` archives and a movie.
pub(crate) fn fake_game_with_files() -> (tempfile::TempDir, GameInstance) {
    use crate::container::big::build_archive;
    let (dir, _) = fake_game();
    let root = dir.path();
    let a0 = build_archive(&[
        ("audiostreams\\a\\one.exa.snu".into(), vec![1; 5000]),
        ("audiostreams\\a\\two.exa.snu".into(), vec![2; 100]),
    ])
    .unwrap();
    let a1 = build_archive(&[("audiostreams\\es\\one.exa.snu".into(), vec![3; 50])]).unwrap();
    fs::write(root.join("GODFATH0.viv"), a0).unwrap();
    fs::write(root.join("GODFATH1.viv"), a1).unwrap();
    fs::create_dir_all(root.join("movies/common")).unwrap();
    fs::write(
        root.join("movies/common/mainmenu.vp6"),
        b"MVhd original movie",
    )
    .unwrap();
    let game = GameInstance::open(root).unwrap();
    (dir, game)
}

fn archive_names(root: &Path) -> Vec<String> {
    let mut names: Vec<String> = crate::container::big::find_archives(root)
        .unwrap()
        .iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[test]
fn archive_entries_go_into_an_overlay_that_the_game_opens_first() {
    use crate::container::big::BigArchive;
    let (dir, game) = fake_game_with_files();
    let root = dir.path();
    let before = snapshot(root);
    let mut ws = Workspace::for_game(&game, None).unwrap();
    let key = FileKey::archive_entry("AudioStreams/a/Two.exa.snu");
    assert_eq!(ws.current_file(&game, &key).unwrap(), vec![2; 100]);
    ws.set_file(&game, &key, vec![9; 3000], "Import two")
        .unwrap();
    assert_eq!(ws.current_file(&game, &key).unwrap(), vec![9; 3000]);
    // Recorded only.
    assert_eq!(snapshot(root), before);
    let status = ws.status(&game).unwrap();
    assert_eq!(status.files, vec![(key.clone(), ChangeState::Pending)]);
    assert!(!status.is_up_to_date());

    let report = ws.apply(&game, ApplyOptions::default()).unwrap();
    assert!(report.written.iter().any(|w| w == "GODFATH0.viv"));
    assert_eq!(
        archive_names(root),
        ["GODFATH0.viv", "GODFATH1.viv", "GODFATH2.viv"]
    );
    // The overlay is number 0: the game finds the new entry first.
    let overlay = BigArchive::open(root.join("GODFATH0.viv")).unwrap();
    assert_eq!(overlay.entries[0].name, files::OVERLAY_MARKER);
    let entry = overlay
        .entries
        .iter()
        .find(|e| e.name == "audiostreams\\a\\two.exa.snu")
        .unwrap();
    assert_eq!(overlay.read(entry).unwrap(), vec![9; 3000]);
    // The game's archives are renamed, not rewritten.
    assert_eq!(
        fs::read(root.join("GODFATH1.viv")).unwrap(),
        before["GODFATH0.viv"]
    );
    assert_eq!(
        fs::read(root.join("GODFATH2.viv")).unwrap(),
        before["GODFATH1.viv"]
    );
    assert_eq!(
        files::original_archives(root).unwrap().len(),
        2,
        "the overlay is not an original"
    );
    let status = ws.status(&game).unwrap();
    assert_eq!(status.files, vec![(key.clone(), ChangeState::Applied)]);
    assert!(status.is_up_to_date());
    // Original data still comes from the game's archives.
    assert_eq!(files::original_data(root, &key).unwrap(), vec![2; 100]);

    // A second entry rewrites only the overlay.
    let other = FileKey::archive_entry("audiostreams\\es\\one.exa.snu");
    ws.set_file(&game, &other, vec![7; 10], "Import es")
        .unwrap();
    ws.apply(&game, ApplyOptions::default()).unwrap();
    assert_eq!(
        BigArchive::open(root.join("GODFATH0.viv"))
            .unwrap()
            .entries
            .len(),
        3
    );
    assert_eq!(archive_names(root).len(), 3);

    // Undo both, apply: the game is back to its original names and data.
    ws.undo().unwrap();
    ws.undo().unwrap();
    assert!(ws.is_empty());
    let status = ws.status(&game).unwrap();
    assert_eq!(status.pending_file_reverts, vec!["GODFATH0.viv".to_owned()]);
    ws.apply(&game, ApplyOptions::default()).unwrap();
    assert_eq!(snapshot(root), before);
    assert!(ws.status(&game).unwrap().is_up_to_date());

    // Redo and unapply: the same.
    ws.redo().unwrap();
    ws.apply(&game, ApplyOptions::default()).unwrap();
    assert_ne!(snapshot(root), before);
    ws.unapply(&game, false).unwrap();
    assert_eq!(snapshot(root), before);
}

#[test]
fn an_interrupted_archive_shift_is_completed_or_undone() {
    let (dir, game) = fake_game_with_files();
    let root = dir.path();
    let before = snapshot(root);
    let mut ws = Workspace::for_game(&game, None).unwrap();
    let key = FileKey::archive_entry("audiostreams\\a\\one.exa.snu");
    ws.set_file(&game, &key, vec![5; 10], "Import").unwrap();
    ws.apply(&game, ApplyOptions::default()).unwrap();
    // Simulate a crash during unapply: overlay removed, one rename undone.
    fs::remove_file(root.join("GODFATH0.viv")).unwrap();
    fs::rename(root.join("GODFATH1.viv"), root.join("GODFATH0.viv")).unwrap();
    ws.unapply(&game, true).unwrap();
    assert_eq!(snapshot(root), before);
}

#[test]
fn loose_files_are_backed_up_and_restored() {
    let (dir, game) = fake_game_with_files();
    let root = dir.path();
    let before = snapshot(root);
    let mut ws = Workspace::for_game(&game, None).unwrap();
    let key: FileKey = "file:movies/common/mainmenu.vp6".parse().unwrap();
    ws.set_file(&game, &key, b"new movie".to_vec(), "Import movie")
        .unwrap();
    // Only the files selected by name are applied in a partial apply.
    ws.apply(
        &game,
        ApplyOptions {
            force: false,
            streams: Some(vec![LEVEL.to_owned()]),
        },
    )
    .unwrap();
    assert_eq!(snapshot(root), before);
    ws.apply(
        &game,
        ApplyOptions {
            force: false,
            streams: Some(vec![FILES.to_owned()]),
        },
    )
    .unwrap();
    assert_eq!(
        fs::read(root.join("movies/common/mainmenu.vp6")).unwrap(),
        b"new movie"
    );
    assert!(
        root.join(crate::game::BACKUP_DIR)
            .join("movies/common/mainmenu.vp6")
            .is_file()
    );
    // The original stays readable for diffs and further imports.
    assert_eq!(
        files::original_data(root, &key).unwrap(),
        b"MVhd original movie"
    );
    // A change made by someone else is a conflict.
    fs::write(root.join("movies/common/mainmenu.vp6"), b"other tool").unwrap();
    assert_eq!(
        ws.status(&game).unwrap().files,
        vec![(key.clone(), ChangeState::Conflict)]
    );
    assert!(ws.unapply(&game, false).is_err());
    ws.unapply(&game, true).unwrap();
    assert_eq!(snapshot(root), before);
    // Setting the original data removes the change.
    ws.set_file(&game, &key, b"MVhd original movie".to_vec(), "Back")
        .unwrap();
    assert!(ws.file_changes().next().is_none());
}

#[test]
fn file_changes_are_kept_in_the_manifest_and_log() {
    let (dir, game) = fake_game_with_files();
    let mut ws = Workspace::for_game(&game, None).unwrap();
    let key = FileKey::archive_entry("audiostreams\\a\\one.exa.snu");
    ws.set_file(&game, &key, vec![1, 2, 3], "Import").unwrap();
    let ws_dir = ws.dir().to_owned();
    drop(ws);
    let manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(ws_dir.join("manifest.json")).unwrap()).unwrap();
    assert_eq!(
        manifest["files"][0]["key"],
        "viv:audiostreams\\a\\one.exa.snu"
    );
    assert_eq!(manifest["files"][0]["original"]["size"], 5000);
    let ws = Workspace::open(&ws_dir).unwrap();
    assert_eq!(ws.current_file(&game, &key).unwrap(), vec![1, 2, 3]);
    assert_eq!(ws.operations()[0].file_edits.len(), 1);
    drop(dir);
}

#[test]
fn operations_of_a_task_join_into_one_or_roll_back() {
    let (dir, game) = fake_game();
    let mut ws = Workspace::for_game(&game, Some(&dir.path().join("ws"))).unwrap();
    let level = stream(&game, LEVEL);
    let text = stream(&game, TEXT);
    let table = |words| sample_table(words);
    ws.set_string_table(&game, level, STRINGS, &table(["Before", "Task"]))
        .unwrap();
    let mark = ws.head();
    assert_eq!(mark, 1);
    // Three steps of a task: the same table twice, another table once.
    ws.set_string_table(&game, level, STRINGS, &table(["Step", "One"]))
        .unwrap();
    ws.set_string_table(&game, text, 0, &table(["Si", "No"]))
        .unwrap();
    ws.set_string_table(&game, level, STRINGS, &table(["Step", "Two"]))
        .unwrap();
    assert_eq!(ws.operations().len(), 4);
    let seq = ws.squash(mark, "Wizard: edit texts").unwrap();
    assert!(seq.is_some());
    assert_eq!(ws.operations().len(), 2);
    assert_eq!(ws.undo_label(), Some("Wizard: edit texts"));
    let strings = key(&game, LEVEL, STRINGS);
    assert_eq!(ws.change(&strings).unwrap().label, "Wizard: edit texts");
    let two = table(["Step", "Two"]).to_bytes().unwrap();
    assert_eq!(ws.data(&strings).unwrap().unwrap(), two);
    // The joined operation survives a restart and undoes in one step.
    let mut ws = Workspace::open(ws.dir()).unwrap();
    assert_eq!(ws.undo().unwrap().as_deref(), Some("Wizard: edit texts"));
    let before = table(["Before", "Task"]).to_bytes().unwrap();
    assert_eq!(ws.data(&strings).unwrap().unwrap(), before);
    assert!(ws.change(&key(&game, TEXT, 0)).is_none());
    assert_eq!(ws.redo().unwrap().as_deref(), Some("Wizard: edit texts"));
    assert_eq!(ws.data(&strings).unwrap().unwrap(), two);

    // Steps that cancel out leave nothing.
    let mark = ws.head();
    ws.set_string_table(&game, text, 0, &table(["Yes", "Maybe"]))
        .unwrap();
    ws.set_string_table(&game, text, 0, &table(["Si", "No"]))
        .unwrap();
    assert_eq!(ws.squash(mark, "Nothing").unwrap(), None);
    assert_eq!(ws.operations().len(), 2);
    let text_key = key(&game, TEXT, 0);
    assert_eq!(ws.change(&text_key).unwrap().label, "Wizard: edit texts");
    assert_eq!(ws.squash(ws.head(), "Empty").unwrap(), None);
    assert!(ws.squash(ws.head() + 1, "Too far").is_err());

    // A task given up: undone, and nothing is left to redo.
    let mark = ws.head();
    ws.set_string_table(&game, level, STRINGS, &table(["Gone", "Soon"]))
        .unwrap();
    ws.set_string_table(&game, text, 0, &table(["Gone", "Too"]))
        .unwrap();
    assert!(ws.rollback(mark).unwrap());
    assert_eq!(ws.operations().len(), mark);
    assert_eq!(ws.redo_label(), None);
    assert_eq!(ws.data(&strings).unwrap().unwrap(), two);
    assert!(!ws.rollback(mark).unwrap());
}

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use super::integrity::{self, Condition, IntegrityOptions};
use super::manager::{Clash, ModApplyOptions, Problem};
use super::package::{ModArchive, write_package};
use super::*;
use crate::workspace::tests::{
    LEVEL, MESH, STRINGS, TEX_DATA, TEXT, edit, fake_game, game_data, key, sample_table, snapshot,
    stream, toc_size,
};
use crate::workspace::{ApplyOptions, NewResourceEdit, PayloadStore, StreamAction};

fn info(id: &str) -> ModInfo {
    ModInfo {
        id: id.to_owned(),
        name: format!("Mod {id}"),
        version: "1.0".to_owned(),
        author: "tester".to_owned(),
        description: "A test mod.\nSecond line.".to_owned(),
        url: Some("https://example.org/mod".to_owned()),
    }
}

/// A workspace in `dir` with the given edits of `gf2/level`.
fn workspace_with(game: &GameInstance, dir: &Path, edits: Vec<(usize, Vec<u8>)>) -> Workspace {
    let mut ws = Workspace::create(dir, game).unwrap();
    let edits: Vec<_> = edits.into_iter().map(|(i, d)| edit(i, d)).collect();
    ws.set_resources(game, "edit", stream(game, LEVEL), &edits)
        .unwrap();
    ws
}

/// Builds a mod package with the given edits of `gf2/level` and returns its path.
fn build_mod(
    game: &GameInstance,
    out_dir: &Path,
    id: &str,
    edits: Vec<(usize, Vec<u8>)>,
) -> PathBuf {
    let ws = workspace_with(game, &out_dir.join(format!("ws-{id}")), edits);
    let out = out_dir.join(format!("{id}.zip"));
    build_from_workspace(&ws, &info(id), None, None, &out).unwrap();
    out
}

fn mesh(byte: u8) -> Vec<u8> {
    vec![byte; 300]
}

fn strings(words: [&str; 2]) -> Vec<u8> {
    sample_table(words).to_bytes().unwrap()
}

// ------------------------------------------------------------------ manifest

#[test]
fn ids_and_names() {
    for good in ["a", "my-mod_2", "0x"] {
        validate_id(good).unwrap();
    }
    for bad in ["", "-a", "A", "a b", "a/b", "..", "a.b", &"a".repeat(65)] {
        assert!(validate_id(bad).is_err(), "{bad}");
    }
    assert_eq!(
        id_from_name("Restored HQ Billboards"),
        "restored-hq-billboards"
    );
    assert_eq!(id_from_name("  US Font Mod!! "), "us-font-mod");
    assert_eq!(id_from_name("Оружие"), "mod");
}

#[test]
fn manifest_validation_rejects_bad_fields() {
    let (_dir, game) = fake_game();
    let temp = tempfile::tempdir().unwrap();
    let ws = workspace_with(&game, &temp.path().join("ws"), vec![(MESH, mesh(1))]);
    let good = manifest_from_workspace(&ws, &info("good"), None).unwrap();
    good.validate().unwrap();
    assert_eq!(ModManifest::parse(&good.to_json().unwrap()).unwrap(), good);

    let mut cases: Vec<(&str, ModManifest)> = Vec::new();
    let mut m = good.clone();
    m.id = "../evil".into();
    cases.push(("bad mod id", m));
    let mut m = good.clone();
    m.name = " ".into();
    cases.push(("`name` is empty", m));
    let mut m = good.clone();
    m.url = Some("file:///etc/passwd".into());
    cases.push(("http", m));
    let mut m = good.clone();
    m.preview = Some("../preview.png".into());
    cases.push(("preview", m));
    let mut m = good.clone();
    m.resources.push(m.resources[0].clone());
    cases.push(("listed twice", m));
    let mut m = good.clone();
    m.resources[0].payload.sha256 = "zz".into();
    cases.push(("not a SHA-256", m));
    let mut m = good.clone();
    m.resources[0].payload.size = MAX_PAYLOAD_SIZE + 1;
    cases.push(("at most", m));
    let mut m = good.clone();
    m.resources.clear();
    cases.push(("no resources", m));
    let mut m = good.clone();
    m.resources[0].key.stream = "gf2/../x".into();
    cases.push(("bad stream", m));
    let mut m = good.clone();
    m.name = "a\u{7}b".into();
    cases.push(("control", m));
    for (expected, manifest) in cases {
        let error = manifest.validate().unwrap_err().to_string();
        assert!(error.contains(expected), "{expected}: {error}");
    }

    // A newer format gets a clear message, not a complaint about a field.
    let mut json: serde_json::Value = serde_json::from_slice(&good.to_json().unwrap()).unwrap();
    json["format_version"] = 99.into();
    json["resources"] = "something new".into();
    let error = ModManifest::parse(json.to_string().as_bytes()).unwrap_err();
    assert!(
        error.to_string().contains("Update doctor_toolkit"),
        "{error}"
    );
    assert!(ModManifest::parse(b"{\"format\":\"other\"}").is_err());
    assert!(ModManifest::parse(b"not json").is_err());
}

// ------------------------------------------------------------------ packages

#[test]
fn builds_and_reads_a_package_with_a_preview() {
    let (_dir, game) = fake_game();
    let temp = tempfile::tempdir().unwrap();
    let ws = workspace_with(
        &game,
        &temp.path().join("ws"),
        vec![(MESH, mesh(1)), (STRINGS, strings(["Hi", "There"]))],
    );
    let png = crate::resource::pixels::encode_png(1, 1, &[255, 0, 0, 255]).unwrap();
    let preview = Preview::from_image(png.clone()).unwrap();
    assert!(Preview::from_image(b"GIF89a".to_vec()).is_err());
    let out = temp.path().join("out.zip");
    let report = build_from_workspace(&ws, &info("two"), None, Some(&preview), &out).unwrap();
    assert_eq!(report.resources, 2);

    let mut archive = ModArchive::open(&out).unwrap();
    assert!(archive.warnings().is_empty(), "{:?}", archive.warnings());
    let manifest = archive.manifest().clone();
    assert_eq!(manifest.id, "two");
    assert_eq!(manifest.game.profile, game.profile().name);
    assert_eq!(manifest.preview.as_deref(), Some("preview.png"));
    assert_eq!(archive.read_preview().unwrap().unwrap(), png);
    archive.verify().unwrap();
    let mesh_key = key(&game, LEVEL, MESH);
    let resource = manifest
        .resources
        .iter()
        .find(|r| r.key == mesh_key)
        .unwrap();
    assert_eq!(archive.read_payload(&resource.payload).unwrap(), mesh(1));
    assert_eq!(resource.original, Blob::of(&game_data(&game, LEVEL, MESH)));

    // Only selected changes.
    let only = temp.path().join("only.zip");
    build_from_workspace(
        &ws,
        &info("one"),
        Some(std::slice::from_ref(&mesh_key)),
        None,
        &only,
    )
    .unwrap();
    assert_eq!(
        ModArchive::open(&only).unwrap().manifest().resources.len(),
        1
    );
    let other = key(&game, TEXT, 0);
    assert!(build_from_workspace(&ws, &info("x"), Some(&[other]), None, &only).is_err());
}

/// Writes a ZIP with the given entries (name, data, deflated).
fn raw_zip(path: &Path, entries: &[(&str, &[u8])]) {
    let file = fs::File::create(path).unwrap();
    let mut zip = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    for (name, data) in entries {
        zip.start_file(*name, options).unwrap();
        zip.write_all(data).unwrap();
    }
    zip.finish().unwrap();
}

/// Replaces every occurrence of `from` by `to` (same length) in a file.
fn patch_bytes(path: &Path, from: &[u8], to: &[u8]) {
    assert_eq!(from.len(), to.len());
    let mut data = fs::read(path).unwrap();
    let mut count = 0;
    let mut i = 0;
    while i + from.len() <= data.len() {
        if &data[i..i + from.len()] == from {
            data[i..i + from.len()].copy_from_slice(to);
            count += 1;
            i += from.len();
        } else {
            i += 1;
        }
    }
    assert!(count > 0, "pattern not found");
    fs::write(path, data).unwrap();
}

/// Sets a field of the local and central headers of entry `entry` (by position).
fn patch_headers(
    path: &Path,
    entry: usize,
    local_offset: usize,
    central_offset: usize,
    value: &[u8],
) {
    let mut data = fs::read(path).unwrap();
    let (mut locals, mut centrals) = (0, 0);
    let mut i = 0;
    while i + 4 <= data.len() {
        let field = match &data[i..i + 4] {
            [0x50, 0x4b, 0x03, 0x04] => {
                locals += 1;
                (locals - 1 == entry).then_some(i + local_offset)
            }
            [0x50, 0x4b, 0x01, 0x02] => {
                centrals += 1;
                (centrals - 1 == entry).then_some(i + central_offset)
            }
            _ => None,
        };
        if let Some(at) = field {
            data[at..at + value.len()].copy_from_slice(value);
        }
        i += 1;
    }
    fs::write(path, data).unwrap();
}

/// A valid manifest for one payload of `data`.
fn manifest_for(game: &GameInstance, data: &[u8]) -> ModManifest {
    let mut manifest = ModManifest::new(
        &info("evil"),
        ModGame {
            profile: game.profile().name.to_owned(),
            executable_size: None,
            executable_sha256: None,
        },
    );
    manifest.resources.push(ModResource {
        key: key(game, LEVEL, MESH),
        payload: Blob::of(data),
        original: Blob::of(&game_data(game, LEVEL, MESH)),
        storage: StorageChoice::Keep,
        label: String::new(),
        added: None,
    });
    manifest
}

#[test]
fn malicious_packages_are_refused() {
    let (_dir, game) = fake_game();
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("evil.zip");
    let payload = mesh(9);
    let manifest = manifest_for(&game, &payload);
    let json = manifest.to_json().unwrap();
    let payload_name = format!("payloads/{}", Blob::of(&payload).sha256);
    let open_error = |path: &Path| ModArchive::open(path).unwrap_err().to_string();

    // The well-formed package opens.
    raw_zip(&path, &[("mod.json", &json), (&payload_name, &payload)]);
    ModArchive::open(&path).unwrap().verify().unwrap();

    // Unsafe names.
    for name in [
        "../evil",
        "/etc/passwd",
        "a\\b",
        "payloads/../../x",
        "c:/x",
        "./mod.json",
    ] {
        raw_zip(&path, &[("mod.json", &json), (name, b"x")]);
        let error = open_error(&path);
        assert!(error.contains("unsafe name"), "{name}: {error}");
    }
    // A bad payload name.
    raw_zip(&path, &[("mod.json", &json), ("payloads/abc", b"x")]);
    assert!(open_error(&path).contains("not named by a SHA-256"));

    // Two entries with the same name: written as `mod.jsoN`, then renamed.
    raw_zip(
        &path,
        &[
            ("mod.json", &json),
            ("mod.jsoN", b"{}"),
            (&payload_name, &payload),
        ],
    );
    patch_bytes(&path, b"mod.jsoN", b"mod.json");
    let error = open_error(&path);
    assert!(error.contains("same name"), "{error}");

    // A missing payload, and a payload whose contents do not match its name.
    raw_zip(&path, &[("mod.json", &json)]);
    assert!(open_error(&path).contains("missing"));
    let mut wrong = payload.clone();
    wrong[0] ^= 1;
    raw_zip(&path, &[("mod.json", &json), (&payload_name, &wrong)]);
    let mut archive = ModArchive::open(&path).unwrap();
    let error = archive.verify().unwrap_err().to_string();
    assert!(error.contains("damaged"), "{error}");

    // A payload larger than the manifest says.
    raw_zip(
        &path,
        &[("mod.json", &json), (&payload_name, &mesh(9).repeat(2))],
    );
    assert!(open_error(&path).contains("mod.json says"));

    // A ZIP bomb: 64 MiB of zeros declared as 300 bytes in both headers.
    // Reading stops at the declared size instead of inflating everything.
    let bomb = vec![0u8; 64 << 20];
    raw_zip(&path, &[("mod.json", &json), (&payload_name, &bomb)]);
    patch_headers(&path, 1, 22, 24, &300u32.to_le_bytes());
    match ModArchive::open(&path) {
        Ok(mut archive) => {
            let error = archive.verify().unwrap_err().to_string();
            assert!(
                error.contains("declare") || error.contains("damaged"),
                "{error}"
            );
        }
        Err(error) => assert!(error.to_string().contains("ZIP"), "{error}"),
    }

    // A manifest that is not JSON, a missing manifest, an unknown compression method.
    raw_zip(
        &path,
        &[("mod.json", b"\xff\xfe"), (&payload_name, &payload)],
    );
    assert!(open_error(&path).contains("not valid JSON"));
    raw_zip(&path, &[(&payload_name, &payload)]);
    assert!(open_error(&path).contains("no mod.json"));
    raw_zip(&path, &[("mod.json", &json), (&payload_name, &payload)]);
    patch_headers(&path, 1, 8, 10, &12u16.to_le_bytes()); // bzip2
    assert!(open_error(&path).contains("compression"));

    // A symbolic link.
    {
        let file = fs::File::create(&path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default();
        zip.start_file("mod.json", options).unwrap();
        zip.write_all(&json).unwrap();
        zip.add_symlink("link", "/etc/passwd", options).unwrap();
        zip.finish().unwrap();
    }
    assert!(open_error(&path).contains("symbolic link"));

    // Not a ZIP at all.
    fs::write(&path, b"PK\x03\x04 garbage").unwrap();
    assert!(open_error(&path).contains("not a valid ZIP"));

    // Unknown but harmless entries are reported, not extracted.
    raw_zip(
        &path,
        &[
            ("mod.json", &json),
            (&payload_name, &payload),
            ("readme.txt", b"hi"),
        ],
    );
    let archive = ModArchive::open(&path).unwrap();
    assert_eq!(archive.warnings().len(), 1);
}

#[test]
fn writing_refuses_payloads_that_do_not_match() {
    let (_dir, game) = fake_game();
    let temp = tempfile::tempdir().unwrap();
    let out = temp.path().join("out.zip");
    let manifest = manifest_for(&game, &mesh(1));
    let error = write_package(&out, &manifest, None, |_| Ok(mesh(2))).unwrap_err();
    assert!(error.to_string().contains("does not match"), "{error}");
    assert!(!out.exists(), "nothing is left behind");
}

// ------------------------------------------------------------------- manager

#[test]
fn installs_applies_orders_and_uninstalls() {
    let (dir, game) = fake_game();
    let root = dir.path();
    let pristine = snapshot(root);
    let temp = tempfile::tempdir().unwrap();
    let a = build_mod(
        &game,
        temp.path(),
        "a",
        vec![(MESH, mesh(1)), (STRINGS, strings(["A", "a"]))],
    );
    let b = build_mod(&game, temp.path(), "b", vec![(MESH, mesh(2))]);

    let mut mods = ModManager::open(&game).unwrap();
    assert!(mods.mods().is_empty());
    let report = mods.install_file(&game, &a, true).unwrap();
    assert_eq!((report.id.as_str(), report.resources), ("a", 2));
    assert!(report.issues.is_empty(), "{:?}", report.issues);
    mods.install_file(&game, &b, true).unwrap();
    assert_eq!(
        snapshot(root),
        pristine,
        "installing does not touch the game"
    );

    // Both change the mesh; b is later and wins.
    let conflicts = mods.conflicts(true);
    assert_eq!(conflicts.len(), 1);
    assert_eq!(conflicts[0].key, key(&game, LEVEL, MESH));
    assert_eq!(conflicts[0].mods, ["a", "b"]);
    assert_eq!(conflicts[0].winner(), "b");
    assert!(!conflicts[0].same_data);

    let report = mods.apply(&game, ModApplyOptions::default()).unwrap();
    assert_eq!(
        report.streams,
        [(LEVEL.to_owned(), StreamAction::Rebuilt(2))]
    );
    assert_eq!(game_data(&game, LEVEL, MESH), mesh(2));
    assert_eq!(game_data(&game, LEVEL, STRINGS), strings(["A", "a"]));
    let level = fs::read(root.join("gf2/level.str")).unwrap();
    assert_eq!(
        toc_size(root, LEVEL) as u64,
        crate::container::ustream::StreamLayout::parse(&level)
            .unwrap()
            .total_data_size()
    );
    let status = mods.status(&game).unwrap();
    assert!(status.streams.iter().all(|s| s.up_to_date));
    assert_eq!(mods.held_streams(&game).unwrap(), [LEVEL]);

    // Reorder: a wins.
    mods.move_mod("a", 1).unwrap();
    assert_eq!(mods.mods()[1].id, "a");
    assert!(!mods.status(&game).unwrap().streams[0].up_to_date);
    mods.apply(&game, ModApplyOptions::default()).unwrap();
    assert_eq!(game_data(&game, LEVEL, MESH), mesh(1));

    // The order and enabled state survive reopening.
    mods.set_enabled("a", false).unwrap();
    let mut mods = ModManager::open(&game).unwrap();
    assert_eq!(
        mods.mods()
            .iter()
            .map(|m| (m.id.as_str(), m.enabled))
            .collect::<Vec<_>>(),
        [("b", true), ("a", false)]
    );
    assert!(mods.conflicts(true).is_empty());
    assert_eq!(mods.conflicts(false).len(), 1);
    mods.apply(&game, ModApplyOptions::default()).unwrap();
    assert_eq!(game_data(&game, LEVEL, MESH), mesh(2));
    assert_eq!(
        game_data(&game, LEVEL, STRINGS),
        sample_table(["Hello", "World"]).to_bytes().unwrap()
    );

    // An update keeps the place and the state.
    let b2 = build_mod(&game, &temp.path().join("v2"), "b", vec![(MESH, mesh(3))]);
    let report = mods.install_file(&game, &b2, false).unwrap();
    assert_eq!(report.replaced_version.as_deref(), Some("1.0"));
    assert_eq!(mods.mods()[0].id, "b");
    assert!(mods.mods()[0].enabled);
    mods.apply(&game, ModApplyOptions::default()).unwrap();
    assert_eq!(game_data(&game, LEVEL, MESH), mesh(3));

    // Uninstalling everything and applying restores the game byte for byte.
    mods.uninstall("a").unwrap();
    mods.uninstall("b").unwrap();
    assert!(mods.uninstall("b").is_err());
    let report = mods.apply(&game, ModApplyOptions::default()).unwrap();
    assert_eq!(report.streams, [(LEVEL.to_owned(), StreamAction::Restored)]);
    assert_eq!(snapshot(root), pristine);
    assert!(!mods.has_written());
    let leftovers: Vec<_> = fs::read_dir(mods.dir())
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    assert!(
        leftovers.iter().all(|n| n.ends_with(".json")),
        "{leftovers:?}"
    );
}

#[test]
fn unapply_restores_originals_and_keeps_the_mods() {
    let (dir, game) = fake_game();
    let pristine = snapshot(dir.path());
    let temp = tempfile::tempdir().unwrap();
    let a = build_mod(&game, temp.path(), "a", vec![(MESH, mesh(1))]);
    let mut mods = ModManager::open(&game).unwrap();
    mods.install_file(&game, &a, true).unwrap();
    mods.apply(&game, ModApplyOptions::default()).unwrap();
    assert_ne!(snapshot(dir.path()), pristine);
    mods.unapply(&game, false).unwrap();
    assert_eq!(snapshot(dir.path()), pristine);
    assert_eq!(mods.mods().len(), 1);
    assert!(!mods.status(&game).unwrap().streams[0].up_to_date);
}

#[test]
fn mods_that_do_not_fit_are_refused() {
    let (dir, game) = fake_game();
    let temp = tempfile::tempdir().unwrap();
    let a = build_mod(&game, temp.path(), "a", vec![(MESH, mesh(1))]);
    let mut mods = ModManager::open(&game).unwrap();
    mods.install_file(&game, &a, true).unwrap();

    // Another tool changes the mesh in the game (no backups): the original
    // the mod expects is no longer there.
    let other = tempfile::tempdir().unwrap();
    let mut ws = Workspace::create(other.path(), &game).unwrap();
    ws.set_resources(&game, "other", stream(&game, LEVEL), &[edit(MESH, mesh(8))])
        .unwrap();
    ws.apply(&game, ApplyOptions::default()).unwrap();
    // Forget the backup, as a tool without backups would leave the game.
    fs::remove_dir_all(dir.path().join("_doctor_toolkit/backups")).unwrap();

    let issues = mods.check(&game).unwrap();
    assert_eq!(issues.len(), 1);
    assert!(matches!(issues[0].problem, Problem::OriginalDiffers { .. }));
    assert!(!issues[0].is_fatal());
    let error = mods.apply(&game, ModApplyOptions::default()).unwrap_err();
    assert!(error.to_string().contains("not the originals"), "{error}");
    let report = mods
        .apply(
            &game,
            ModApplyOptions {
                ignore_compatibility: true,
                ..Default::default()
            },
        )
        .unwrap();
    assert!(!report.warnings.is_empty());
    assert_eq!(game_data(&game, LEVEL, MESH), mesh(1));

    // A mod for a resource the game does not have cannot be applied at all.
    let mut manifest = manifest_for(&game, &mesh(5));
    manifest.id = "ghost".into();
    manifest.resources[0].key.ids = [0xdead; 4];
    let ghost = temp.path().join("ghost.zip");
    write_package(&ghost, &manifest, None, |_| Ok(mesh(5))).unwrap();
    let report = mods.install_file(&game, &ghost, true).unwrap();
    assert!(
        report
            .issues
            .iter()
            .all(|i| i.problem == Problem::ResourceMissing)
    );
    let error = mods
        .apply(
            &game,
            ModApplyOptions {
                ignore_compatibility: true,
                force: true,
            },
        )
        .unwrap_err();
    assert!(
        error.to_string().contains("do not fit this game"),
        "{error}"
    );

    // A mod for another game is not installed.
    let mut manifest = manifest_for(&game, &mesh(5));
    manifest.game.profile = "Dead Space (PC)".into();
    let foreign = temp.path().join("foreign.zip");
    write_package(&foreign, &manifest, None, |_| Ok(mesh(5))).unwrap();
    let error = mods.install_file(&game, &foreign, true).unwrap_err();
    assert!(error.to_string().contains("made for Dead Space"), "{error}");

    // Neither is a file that is not a mod.
    let junk = temp.path().join("junk.bin");
    fs::write(&junk, b"hello world").unwrap();
    assert!(mods.install_file(&game, &junk, true).is_err());
    assert_eq!(mods.mods().len(), 2);
}

#[test]
fn a_damaged_installed_mod_is_kept_but_cannot_be_applied() {
    let (_dir, game) = fake_game();
    let temp = tempfile::tempdir().unwrap();
    let a = build_mod(&game, temp.path(), "a", vec![(MESH, mesh(1))]);
    let mut mods = ModManager::open(&game).unwrap();
    mods.install_file(&game, &a, true).unwrap();
    fs::write(mods.mods()[0].dir().join(MANIFEST_NAME), b"{").unwrap();
    let mut mods = ModManager::open(&game).unwrap();
    assert!(mods.mods()[0].problem.is_some());
    assert!(!mods.mods()[0].enabled);
    assert!(mods.set_enabled("a", true).is_err());
    mods.uninstall("a").unwrap();
    assert!(mods.mods().is_empty());
}

#[test]
fn the_workspace_and_the_mods_do_not_write_at_the_same_time() {
    let (dir, game) = fake_game();
    let pristine = snapshot(dir.path());
    let temp = tempfile::tempdir().unwrap();
    let a = build_mod(&game, temp.path(), "a", vec![(MESH, mesh(1))]);
    let mut mods = ModManager::open(&game).unwrap();
    mods.install_file(&game, &a, true).unwrap();

    // The editor's workspace has its change in the game: the mods wait.
    let mut ws = Workspace::for_game(&game, None).unwrap();
    ws.set_resources(
        &game,
        "edit",
        stream(&game, LEVEL),
        &[edit(STRINGS, strings(["W", "w"]))],
    )
    .unwrap();
    ws.apply(&game, ApplyOptions::default()).unwrap();
    let error = mods.apply(&game, ModApplyOptions::default()).unwrap_err();
    assert!(
        error.to_string().contains("workspace has its changes"),
        "{error}"
    );
    let error = mods
        .apply(
            &game,
            ModApplyOptions {
                force: true,
                ignore_compatibility: true,
            },
        )
        .unwrap_err();
    assert!(
        error.to_string().contains("workspace"),
        "force does not override"
    );

    // The workspace goes out, the mods come in; now the workspace waits.
    ws.unapply(&game, false).unwrap();
    mods.apply(&game, ModApplyOptions::default()).unwrap();
    let error = ws.apply(&game, ApplyOptions::default()).unwrap_err();
    assert!(
        error.to_string().contains("mod manager has mods"),
        "{error}"
    );
    assert_eq!(game_data(&game, LEVEL, MESH), mesh(1));

    // The author installs the workspace as a mod instead: it goes last and wins.
    let report = mods
        .install_workspace(&game, &ws, &info("my-work"), None)
        .unwrap();
    assert_eq!(report.resources, 1);
    assert_eq!(mods.mods().last().unwrap().id, "my-work");
    mods.apply(&game, ModApplyOptions::default()).unwrap();
    assert_eq!(game_data(&game, LEVEL, STRINGS), strings(["W", "w"]));
    assert_eq!(game_data(&game, LEVEL, MESH), mesh(1));

    // Restoring the originals lets the workspace write again.
    mods.unapply(&game, false).unwrap();
    assert_eq!(snapshot(dir.path()), pristine);
    ws.apply(&game, ApplyOptions::default()).unwrap();
    assert_eq!(game_data(&game, LEVEL, STRINGS), strings(["W", "w"]));
}

// ---------------------------------------------------------------------- vpat

/// One resource of a test patch: type id, ids (`None`: addressed by type),
/// operation, data.
type VpatEntry = (u32, Option<[u32; 4]>, u32, Vec<u8>);

/// Writes a `.vpat` file: (path in the patch, file flags, resources). A file
/// with resources addressed by type gets flag 2 whatever `flags` says.
fn write_vpat_full(
    path: &Path,
    name: &str,
    patch_flags: u32,
    files: &[(&str, u32, Vec<VpatEntry>)],
) {
    let put = |out: &mut Vec<u8>, value: u32| out.extend_from_slice(&value.to_le_bytes());
    let mut out = Vec::new();
    out.extend_from_slice(b"VPAT");
    put(&mut out, 1);
    for text in [name, "someone"] {
        out.extend_from_slice(text.as_bytes());
        out.push(0);
    }
    for value in [3u32, 2, 0x1234_5678, patch_flags, files.len() as u32] {
        put(&mut out, value);
    }
    for (file, flags, resources) in files {
        out.extend_from_slice(file.as_bytes());
        out.push(0);
        let (by_ids, by_type): (Vec<_>, Vec<_>) = resources.iter().partition(|r| r.1.is_some());
        let flags = if by_type.is_empty() {
            *flags
        } else {
            flags | vpat::FILE_FLAG_BY_TYPE
        };
        for value in [0u32, flags, by_ids.len() as u32] {
            put(&mut out, value);
        }
        for (type_id, ids, operation, data) in by_ids {
            for value in ids.unwrap() {
                put(&mut out, value);
            }
            for value in [*type_id, *operation, data.len() as u32] {
                put(&mut out, value);
            }
            out.extend_from_slice(data);
        }
        if !by_type.is_empty() {
            out.push(by_type.len() as u8);
            for (type_id, _, operation, data) in by_type {
                for value in [*type_id, *operation, data.len() as u32] {
                    put(&mut out, value);
                }
                out.extend_from_slice(data);
            }
        }
    }
    fs::write(path, out).unwrap();
}

/// Writes a `.vpat` file as the old toolkit did: every resource replaced by ids.
/// A resource replaced by ids in an old-style patch: ids, type id, data.
type OldVpatEntry = ([u32; 4], u32, Vec<u8>);

fn write_vpat(path: &Path, name: &str, files: &[(&str, Vec<OldVpatEntry>)]) {
    let files: Vec<_> = files
        .iter()
        .map(|(file, resources)| {
            let resources = resources
                .iter()
                .map(|(ids, type_id, data)| (*type_id, Some(*ids), 1, data.clone()))
                .collect();
            (*file, 1, resources)
        })
        .collect();
    write_vpat_full(path, name, 0, &files);
}

fn vpat_resource(game: &GameInstance, index: usize, data: Vec<u8>) -> OldVpatEntry {
    let key = key(game, LEVEL, index);
    (key.ids, key.type_id, data)
}

/// The fake game with more resources at the end of `gf2/level`: (type tag, ids, data).
fn fake_game_with(extra: &[(&[u8], [u32; 4], Vec<u8>)]) -> (tempfile::TempDir, GameInstance) {
    use crate::container::ustream::{FileHeader, Storage, StreamLayout, StreamWriter};
    let (dir, game) = fake_game();
    let root = dir.path();
    let file = game.open_stream(stream(&game, LEVEL)).unwrap();
    let mut writer = StreamWriter::new(&FileHeader::GF2_PC, crate::io::Endian::Little);
    for (i, resource) in file.layout().resources.iter().enumerate() {
        writer
            .push_resource(
                &resource.header,
                &file.resource_data(i).unwrap(),
                Storage::Raw,
            )
            .unwrap();
    }
    for (tag, ids, data) in extra {
        let header = crate::workspace::tests::header(tag, *ids);
        writer.push_resource(&header, data, Storage::Raw).unwrap();
    }
    let level = writer.finish();
    drop(file);
    fs::write(root.join("gf2/level.str"), &level).unwrap();
    let size = |data: &[u8]| StreamLayout::parse(data).unwrap().total_data_size();
    let text = fs::read(root.join("gf2/text/abcd.str")).unwrap();
    let toc = crate::workspace::tests::toc_bytes(&[(LEVEL, size(&level)), (TEXT, size(&text))]);
    let toc = crate::toc::Toc::parse(&toc).unwrap();
    fs::write(root.join("gf2/level.toc"), toc.to_bytes().unwrap()).unwrap();
    let game = GameInstance::open(root).unwrap();
    (dir, game)
}

/// The fake game with a `lighttoc` with zero ids, addressed by type in patches.
fn fake_game_with_zero_ids() -> (tempfile::TempDir, GameInstance) {
    fake_game_with(&[(b"lighttoc", vpat::BY_TYPE_IDS, vec![5; 16])])
}

#[test]
fn vpat_patches_are_read_and_installed() {
    let (_dir, game) = fake_game();
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("My Patch.vpat");
    write_vpat(
        &path,
        "My Patch",
        &[
            (
                "gf2\\level.str",
                vec![
                    vpat_resource(&game, MESH, mesh(4)),
                    vpat_resource(&game, TEX_DATA, vec![1; 8]),
                    ([2; 4], 1, vec![0; 4]),
                ],
            ),
            ("gf2\\missing.str", vec![([2; 4], 1, vec![0; 4])]),
        ],
    );
    let patch = vpat::VpatPatch::open(&path).unwrap();
    assert_eq!(patch.name, "My Patch");
    assert_eq!(patch.author, "someone");
    assert_eq!((patch.version, patch.game, patch.flags), (3, 2, 0));
    assert_eq!(patch.files[0].stream, LEVEL);
    assert_eq!(
        (patch.files[0].stream_version, patch.files[0].flags),
        (0, 1)
    );
    assert_eq!(
        patch.files[0].resources[0].operation,
        vpat::VpatOperation::Modify
    );
    assert!(!patch.files[0].resources[0].by_type);
    assert_eq!(patch.resource_count(), 4);
    assert_eq!(
        patch.read_data(&patch.files[0].resources[0]).unwrap(),
        mesh(4)
    );

    let mut mods = ModManager::open(&game).unwrap();
    let report = mods.install_file(&game, &path, true).unwrap();
    assert_eq!(report.id, "my-patch");
    assert_eq!(report.resources, 2);
    assert_eq!(report.warnings.len(), 2, "{:?}", report.warnings);
    let installed = &mods.mods()[0];
    assert_eq!(installed.manifest.converted_from.as_deref(), Some("vpat"));
    assert_eq!(installed.manifest.version, "3");
    assert_eq!(installed.manifest.format_version, 1, "replacements only");
    mods.apply(&game, ModApplyOptions::default()).unwrap();
    assert_eq!(game_data(&game, LEVEL, MESH), mesh(4));
    assert_eq!(game_data(&game, LEVEL, TEX_DATA), vec![1; 8]);
}

/// Resources addressed by type, added resources and operations other than
/// modify, as another EARS editor writes them.
#[test]
fn vpat_resources_by_type_and_added_resources_install() {
    let (_dir, game) = fake_game_with_zero_ids();
    let lighttoc = crate::hash::sdbm(b"lighttoc");
    let variabledictionary = crate::hash::sdbm(b"variabledictionary");
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("by-type.vpat");
    let mesh_key = key(&game, LEVEL, MESH);
    write_vpat_full(
        &path,
        "By type",
        vpat::PATCH_FLAG_INCREMENT_VERSION,
        &[(
            "gf2\\level.str",
            1,
            vec![
                // Replaced by ids, added by ids, skipped (operation 0).
                (mesh_key.type_id, Some(mesh_key.ids), 1, mesh(9)),
                (mesh_key.type_id, Some([0x77; 4]), 3, mesh(8)),
                (mesh_key.type_id, Some([0x78; 4]), 0, mesh(7)),
                // Replaced by type, added by type, by type but not in the stream.
                (lighttoc, None, 1, vec![6; 16]),
                (variabledictionary, None, 3, vec![1, 2, 3]),
                (crate::hash::sdbm(b"bricknmortarres"), None, 1, vec![1]),
            ],
        )],
    );
    let patch = vpat::VpatPatch::open(&path).unwrap();
    assert_eq!(patch.flags, vpat::PATCH_FLAG_INCREMENT_VERSION);
    let file = &patch.files[0];
    assert_eq!(file.flags, vpat::FILE_FLAG_NONE | vpat::FILE_FLAG_BY_TYPE);
    let ops: Vec<_> = file
        .resources
        .iter()
        .map(|r| (r.operation, r.by_type, r.ids))
        .collect();
    use vpat::VpatOperation as Op;
    assert_eq!(
        ops,
        [
            (Op::Modify, false, mesh_key.ids),
            (Op::Add, false, [0x77; 4]),
            (Op::None, false, [0x78; 4]),
            (Op::Modify, true, [0; 4]),
            (Op::Add, true, [0; 4]),
            (Op::Modify, true, [0; 4]),
        ]
    );

    let mut mods = ModManager::open(&game).unwrap();
    let report = mods.install_file(&game, &path, true).unwrap();
    assert_eq!((report.resources, report.added), (4, 2), "{report:?}");
    assert_eq!(report.warnings.len(), 2, "{:?}", report.warnings);
    let manifest = &mods.mods()[0].manifest;
    assert_eq!(manifest.format_version, ADDING_VERSION);
    let added = manifest
        .resources
        .iter()
        .find(|r| r.key.ids == [0x77; 4])
        .unwrap();
    let added_info = added.added.as_ref().unwrap();
    // The policy and alignment of the mesh already in the stream.
    assert_eq!(added_info.mem_policy, crate::hash::sdbm(b"default"));
    assert_eq!(added_info.before, None);
    mods.apply(&game, ModApplyOptions::default()).unwrap();

    let file = game.open_stream(stream(&game, LEVEL)).unwrap();
    let data_of = |type_id: u32, ids: [u32; 4]| {
        let i = file
            .layout()
            .resources
            .iter()
            .position(|r| r.header.type_id == type_id && r.header.ids == ids)
            .unwrap();
        file.resource_data(i).unwrap()
    };
    assert_eq!(data_of(mesh_key.type_id, mesh_key.ids), mesh(9));
    assert_eq!(data_of(mesh_key.type_id, [0x77; 4]), mesh(8));
    assert_eq!(data_of(lighttoc, [0; 4]), vec![6; 16]);
    assert_eq!(data_of(variabledictionary, [0; 4]), vec![1, 2, 3]);
    assert!(
        !file
            .layout()
            .resources
            .iter()
            .any(|r| r.header.ids == [0x78; 4])
    );
}

#[test]
fn vpat_removals_and_ambiguous_types_are_refused() {
    let (_dir, game) = fake_game_with_zero_ids();
    let temp = tempfile::tempdir().unwrap();
    let store = PayloadStore::new(temp.path().join("store"));
    let path = temp.path().join("remove.vpat");
    let mesh_key = key(&game, LEVEL, MESH);
    write_vpat_full(
        &path,
        "Remove",
        0,
        &[(
            "gf2\\level.str",
            1,
            vec![
                (mesh_key.type_id, Some(mesh_key.ids), 1, mesh(9)),
                (mesh_key.type_id, Some(mesh_key.ids), 2, Vec::new()),
            ],
        )],
    );
    let patch = vpat::VpatPatch::open(&path).unwrap();
    assert_eq!(
        patch.files[0].resources[1].operation,
        vpat::VpatOperation::Remove
    );
    let error = patch.to_mod(&game, &store).unwrap_err().to_string();
    assert!(error.contains("removes 1 resource"), "{error}");
    assert!(error.contains(&mesh_key.to_string()), "{error}");

    // Two resources of the type with zero ids: addressing by type is ambiguous.
    let (_dir2, game2) = fake_game_with(&[
        (b"lighttoc", vpat::BY_TYPE_IDS, vec![5; 16]),
        (b"lighttoc", vpat::BY_TYPE_IDS, vec![1; 4]),
    ]);
    write_vpat_full(
        &path,
        "Ambiguous",
        0,
        &[(
            "gf2\\level.str",
            1,
            vec![(crate::hash::sdbm(b"lighttoc"), None, 1, vec![3; 4])],
        )],
    );
    let patch = vpat::VpatPatch::open(&path).unwrap();
    let error = patch.to_mod(&game2, &store).unwrap_err().to_string();
    assert!(error.contains("2 resources of type lighttoc"), "{error}");
}

#[test]
fn malformed_vpat_patches_are_refused() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("p.vpat");
    write_vpat(
        &path,
        "P",
        &[("gf2\\level.str", vec![([2; 4], 1, vec![7; 100])])],
    );
    let good = fs::read(&path).unwrap();
    vpat::VpatPatch::open(&path).unwrap();
    for cut in [3, 10, 30, good.len() - 1] {
        fs::write(&path, &good[..cut]).unwrap();
        assert!(vpat::VpatPatch::open(&path).is_err(), "cut at {cut}");
    }
    let mut extra = good.clone();
    extra.push(0);
    fs::write(&path, &extra).unwrap();
    assert!(vpat::VpatPatch::open(&path).is_err(), "trailing data");
    // A size far beyond the file.
    let mut huge = good.clone();
    let at = good.len() - 100 - 4;
    huge[at..at + 4].copy_from_slice(&u32::MAX.to_le_bytes());
    fs::write(&path, &huge).unwrap();
    assert!(vpat::VpatPatch::open(&path).is_err());
    // An unknown operation.
    let mut bad_op = good.clone();
    let at = good.len() - 100 - 8;
    bad_op[at..at + 4].copy_from_slice(&4u32.to_le_bytes());
    fs::write(&path, &bad_op).unwrap();
    let error = vpat::VpatPatch::open(&path).unwrap_err().to_string();
    assert!(error.contains("unknown operation 4"), "{error}");
    // Unknown flags of a file.
    let file_flags = 4 + 4 + 2 + 8 + 20 + "gf2\\level.str".len() + 1 + 4;
    let mut bad_flags = good.clone();
    bad_flags[file_flags..file_flags + 4].copy_from_slice(&8u32.to_le_bytes());
    fs::write(&path, &bad_flags).unwrap();
    assert!(vpat::VpatPatch::open(&path).is_err());
    // Flag 2 promises a block of resources addressed by type that is missing.
    let mut no_block = good.clone();
    no_block[file_flags..file_flags + 4].copy_from_slice(&3u32.to_le_bytes());
    fs::write(&path, &no_block).unwrap();
    assert!(vpat::VpatPatch::open(&path).is_err());
    // … and with the block it reads.
    no_block.push(0);
    fs::write(&path, &no_block).unwrap();
    assert_eq!(vpat::VpatPatch::open(&path).unwrap().files[0].flags, 3);
    // Another game.
    let mut other = good;
    let at = 4 + 4 + 2 + 8 + 4;
    other[at..at + 4].copy_from_slice(&7u32.to_le_bytes());
    fs::write(&path, &other).unwrap();
    let patch = vpat::VpatPatch::open(&path).unwrap();
    assert_eq!(patch.game, 7);
    let (_dir, game) = fake_game();
    let store = PayloadStore::new(temp.path().join("store"));
    assert!(patch.to_mod(&game, &store).is_err());
}

// ----------------------------------------------------------------- integrity

#[test]
fn integrity_names_who_changed_what() {
    let (dir, game) = fake_game();
    let root = dir.path();
    let none = |_: usize, _: usize| {};
    let report = integrity::check(&game, IntegrityOptions::default(), none).unwrap();
    assert_eq!(report.files.len(), 3);
    assert_eq!(report.count(Condition::Unverified), 3);
    assert!(report.old_toolkit.is_none());

    // A reference list made now makes every file verifiable.
    assert_eq!(integrity::save_reference(&game, none).unwrap(), 3);
    let full = IntegrityOptions { full: true };
    let report = integrity::check(&game, full, none).unwrap();
    assert_eq!(report.count(Condition::Original), 3);

    // Mods written by the manager.
    let temp = tempfile::tempdir().unwrap();
    let a = build_mod(&game, temp.path(), "a", vec![(MESH, vec![1; 400])]);
    let mut mods = ModManager::open(&game).unwrap();
    mods.install_file(&game, &a, true).unwrap();
    mods.apply(&game, ModApplyOptions::default()).unwrap();
    let report = integrity::check(&game, full, none).unwrap();
    let conditions: Vec<_> = report
        .files
        .iter()
        .map(|f| (f.rel_path.as_str(), f.condition))
        .collect();
    assert_eq!(
        conditions,
        [
            ("gf2/level.str", Condition::Mods),
            ("gf2/level.toc", Condition::Mods),
            ("gf2/text/abcd.str", Condition::Original),
        ]
    );

    // Another program rewrites a file the toolkit has never touched.
    let text = root.join("gf2/text/abcd.str");
    let mut data = fs::read(&text).unwrap();
    let last = data.len() - 1;
    data[last] ^= 0xff;
    fs::write(&text, &data).unwrap();
    let report = integrity::check(&game, full, none).unwrap();
    assert_eq!(report.files[2].condition, Condition::DiffersFromReference);
    let quick = integrity::check(&game, IntegrityOptions::default(), none).unwrap();
    assert_eq!(
        quick.files[2].condition,
        Condition::Unverified,
        "same size, not hashed"
    );

    // ... and one the mod manager wrote.
    let level = root.join("gf2/level.str");
    let mut data = fs::read(&level).unwrap();
    let last = data.len() - 1;
    data[last] ^= 0xff;
    fs::write(&level, &data).unwrap();
    let report = integrity::check(&game, full, none).unwrap();
    assert_eq!(report.files[0].condition, Condition::OtherProgram);
    assert!(report.temp_files.is_empty());
}

/// A game patched by the old toolkit: its backup holds the original, the
/// game file the patched version, and the patch is listed as installed.
#[test]
fn a_game_patched_by_the_old_toolkit_is_taken_over() {
    let (dir, game) = fake_game();
    let root = dir.path();
    let pristine = snapshot(root);
    let old = root.join("_toolkit");
    fs::create_dir_all(old.join("backups/gf2")).unwrap();
    fs::create_dir_all(old.join("installed_patches")).unwrap();
    fs::copy(
        root.join("gf2/level.str"),
        old.join("backups/gf2/level.str"),
    )
    .unwrap();
    write_vpat(
        &old.join("installed_patches/Old Mesh.vpat"),
        "Old Mesh",
        &[("gf2\\level.str", vec![vpat_resource(&game, MESH, mesh(6))])],
    );
    fs::write(
        old.join("game_instance.xml"),
        "<instance><installed_patches><patch>Old Mesh.vpat</patch></installed_patches></instance>",
    )
    .unwrap();
    // The old toolkit's result: the mesh replaced, the table of contents untouched.
    let other = tempfile::tempdir().unwrap();
    let mut ws = Workspace::create(other.path(), &game).unwrap();
    ws.set_resources(&game, "old", stream(&game, LEVEL), &[edit(MESH, mesh(6))])
        .unwrap();
    ws.apply(&game, ApplyOptions::default()).unwrap();
    fs::remove_dir_all(root.join("_doctor_toolkit")).unwrap();
    fs::write(root.join("gf2/level.toc"), &pristine["gf2/level.toc"]).unwrap();
    let game = GameInstance::open(root).unwrap();

    let none = |_: usize, _: usize| {};
    let report = integrity::check(&game, IntegrityOptions::default(), none).unwrap();
    assert_eq!(report.files[0].condition, Condition::OldToolkit);
    let old_toolkit = report.old_toolkit.unwrap();
    assert_eq!(old_toolkit.installed_patches, ["Old Mesh.vpat"]);
    assert_eq!(old_toolkit.backups, ["gf2/level.str"]);

    // A converted patch takes its originals from the old backup.
    let mut mods = ModManager::open(&game).unwrap();
    let import = mods.import_old_toolkit(&game).unwrap();
    assert_eq!(import.adopted, ["gf2/level.str"]);
    let (name, result) = &import.installed[0];
    assert_eq!(name, "Old Mesh.vpat");
    let installed = result.as_ref().unwrap();
    assert!(installed.issues.is_empty(), "{:?}", installed.issues);
    let resource = &mods.mods()[0].manifest.resources[0];
    assert_eq!(resource.original, Blob::of(&mesh(7)));

    // The patched file now differs from the adopted original: applying
    // needs force and leaves the game in the same state, with a correct TOC.
    let error = mods.apply(&game, ModApplyOptions::default()).unwrap_err();
    assert!(error.to_string().contains("another program"), "{error}");
    mods.apply(
        &game,
        ModApplyOptions {
            force: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(game_data(&game, LEVEL, MESH), mesh(6));
    let report = integrity::check(&game, IntegrityOptions::default(), none).unwrap();
    assert_eq!(report.files[0].condition, Condition::Mods);
    // Restoring the originals now gives the unpatched game.
    mods.unapply(&game, false).unwrap();
    let mut now = snapshot(root);
    now.retain(|path, _| !path.starts_with("_toolkit/"));
    assert_eq!(now, pristine);
}

// ------------------------------------------- added resources and new streams

const PROPS: &str = "gf2/doctor/props";

fn new_stream_def(name: &str, hash: u32, with_parent: bool) -> NewStream {
    NewStream {
        name: name.into(),
        guid: format!("{hash:08x}{}", "0".repeat(24)),
        hash,
        parent: LEVEL.into(),
        with_parent,
        tocs: vec!["gf2/level.toc".into()],
        toc_parents: Default::default(),
        label: String::new(),
    }
}

fn added(stream: &str, tag: &[u8], id: u32, data: &[u8]) -> NewResourceEdit {
    let mut header = crate::workspace::tests::header(tag, [id; 4]);
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

/// A workspace in `dir` that creates `streams` and adds `resources`.
fn adding_workspace(
    game: &GameInstance,
    dir: &Path,
    streams: Vec<NewStream>,
    resources: Vec<NewResourceEdit>,
) -> Workspace {
    let mut ws = Workspace::create(dir, game).unwrap();
    for stream in streams {
        ws.add_stream(game, stream).unwrap();
    }
    if !resources.is_empty() {
        ws.add_resources(game, "add", &resources).unwrap();
    }
    ws
}

/// Builds a mod package from an adding workspace and returns its path.
fn build_adding_mod(
    game: &GameInstance,
    out_dir: &Path,
    id: &str,
    streams: Vec<NewStream>,
    resources: Vec<NewResourceEdit>,
) -> PathBuf {
    let ws = adding_workspace(game, &out_dir.join(format!("ws-{id}")), streams, resources);
    let out = out_dir.join(format!("{id}.zip"));
    build_from_workspace(&ws, &info(id), None, None, &out).unwrap();
    out
}

/// Writes a package for `manifest` whose payloads are among `data`.
fn write_mod(path: &Path, manifest: &ModManifest, data: &[Vec<u8>]) {
    write_package(path, manifest, None, |blob| {
        Ok(data
            .iter()
            .find(|d| Blob::of(d) == *blob)
            .expect("payload given")
            .clone())
    })
    .unwrap();
}

#[test]
fn version_2_manifests_round_trip_and_version_1_still_reads() {
    let (_dir, game) = fake_game();
    let temp = tempfile::tempdir().unwrap();
    let mut first = added(LEVEL, b"metamodel", 0x1234, b"before the mesh");
    first.before = Some(key(&game, LEVEL, MESH));
    let ws = adding_workspace(
        &game,
        &temp.path().join("ws"),
        vec![new_stream_def(PROPS, 0x0777_0000, true)],
        vec![first, added(PROPS, b"mesh", 0x5678, &[5u8; 64])],
    );
    let manifest = manifest_from_workspace(&ws, &info("adds"), None).unwrap();
    assert_eq!(manifest.format_version, 2);
    assert_eq!(manifest.added().count(), 2);
    assert_eq!(manifest.streams.len(), 1);
    assert_eq!(manifest.streams(), [PROPS, LEVEL]);
    let json = manifest.to_json().unwrap();
    let text = String::from_utf8(json.clone()).unwrap();
    assert!(text.contains("\"added\"") && text.contains("\"mem_policy\""));
    assert!(text.contains("\"with_parent\": true"));
    assert_eq!(ModManifest::parse(&json).unwrap(), manifest);

    // Selecting only the resource of the new stream takes the stream along.
    let only = [key_of(&manifest, PROPS)];
    let part = manifest_from_workspace(&ws, &info("part"), Some(&only)).unwrap();
    assert_eq!(part.resources.len(), 1);
    assert_eq!(part.streams.len(), 1);
    // A mod that only replaces is still written as version 1, without new fields.
    let replacing = workspace_with(&game, &temp.path().join("ws1"), vec![(MESH, mesh(1))]);
    let v1 = manifest_from_workspace(&replacing, &info("v1"), None).unwrap();
    assert_eq!(v1.format_version, 1);
    let text = String::from_utf8(v1.to_json().unwrap()).unwrap();
    assert!(
        !text.contains("added") && !text.contains("streams"),
        "{text}"
    );
    // A version 1 manifest as the previous program wrote it.
    let mut old: serde_json::Value = serde_json::from_str(&text).unwrap();
    old.as_object_mut().unwrap().remove("converted_from");
    assert_eq!(ModManifest::parse(old.to_string().as_bytes()).unwrap(), v1);
    // Version 1 cannot carry added resources.
    let mut stamped = manifest.clone();
    stamped.format_version = 1;
    let error = stamped.validate().unwrap_err().to_string();
    assert!(error.contains("version 2 is needed"), "{error}");
}

fn key_of(manifest: &ModManifest, stream: &str) -> ResourceKey {
    manifest
        .resources
        .iter()
        .find(|r| r.key.stream == stream)
        .unwrap()
        .key
        .clone()
}

#[test]
fn adding_mods_install_apply_and_restore_byte_for_byte() {
    for with_parent in [false, true] {
        let (dir, game) = fake_game();
        let root = dir.path();
        let pristine = snapshot(root);
        let temp = tempfile::tempdir().unwrap();
        let mut first = added(LEVEL, b"metamodel", 0x1234, b"before the mesh");
        first.before = Some(key(&game, LEVEL, MESH));
        let table = strings(["New", "Stream"]);
        let path = build_adding_mod(
            &game,
            temp.path(),
            "adds",
            vec![new_stream_def(PROPS, 0x0777_0000, with_parent)],
            vec![
                first,
                added(PROPS, b"lh2", 0xAA, &table),
                added(PROPS, b"mesh", 0xBB, &[5u8; 1000]),
            ],
        );
        let mut mods = ModManager::open(&game).unwrap();
        let report = mods.install_file(&game, &path, true).unwrap();
        assert_eq!(
            (report.resources, report.added, report.new_streams),
            (3, 3, 1)
        );
        assert!(report.issues.is_empty(), "{:?}", report.issues);
        assert_eq!(
            snapshot(root),
            pristine,
            "installing does not touch the game"
        );
        assert!(mods.clashes(true).is_empty());

        mods.apply(&game, ModApplyOptions::default()).unwrap();
        let status = mods.status(&game).unwrap();
        assert!(status.streams.iter().all(|s| s.up_to_date), "{status:?}");
        let mut held = mods.held_streams(&game).unwrap();
        held.sort();
        assert_eq!(held, [PROPS, LEVEL]);
        // The level has the metamodel before the mesh; the new stream its two resources.
        let level = game.open_stream(stream(&game, LEVEL)).unwrap();
        assert_eq!(level.resource_data(MESH).unwrap(), b"before the mesh");
        assert_eq!(level.layout().resources[MESH].header.ids, [0x1234; 4]);
        assert_eq!(
            toc_size(root, LEVEL) as u64,
            level.layout().total_data_size()
        );
        drop(level);
        let created = fs::read(root.join("gf2/doctor/props.str")).unwrap();
        let layout = crate::container::ustream::StreamLayout::parse(&created).unwrap();
        assert_eq!(layout.resources.len(), 2);
        assert_eq!(layout.resources[0].header.ids, [0xAA; 4]);
        let toc = crate::toc::Toc::parse(&fs::read(root.join("gf2/level.toc")).unwrap()).unwrap();
        let entry = toc.find_file(PROPS).unwrap();
        assert_eq!(toc.file_name(entry).hash, 0x0777_0000);
        assert_eq!(toc.files[entry].data_size as u64, layout.total_data_size());
        let flags = if with_parent {
            crate::toc::FLAGS_PARENTED
        } else {
            crate::toc::FLAGS_REQUESTED
        };
        assert_eq!(toc.files[entry].flags, flags);

        // The integrity check names the mod manager for the created stream too.
        let none = |_: usize, _: usize| {};
        let game_now = GameInstance::open(root).unwrap();
        let report = integrity::check(&game_now, IntegrityOptions::default(), none).unwrap();
        let condition = |rel: &str| {
            report
                .files
                .iter()
                .find(|f| f.rel_path == rel)
                .unwrap()
                .condition
        };
        assert_eq!(condition("gf2/doctor/props.str"), Condition::Mods);
        assert_eq!(condition("gf2/level.toc"), Condition::Mods);
        // Checking again with the created stream in the game is still clean.
        assert!(mods.check(&game_now).unwrap().is_empty());
        mods.apply(&game_now, ModApplyOptions::default()).unwrap();

        // Unapply, apply, disable and uninstall all return the exact originals.
        mods.unapply(&game_now, false).unwrap();
        assert_eq!(snapshot(root), pristine);
        mods.apply(&game_now, ModApplyOptions::default()).unwrap();
        assert_ne!(snapshot(root), pristine);
        mods.set_enabled("adds", false).unwrap();
        let game_now = GameInstance::open(root).unwrap();
        mods.apply(&game_now, ModApplyOptions::default()).unwrap();
        assert_eq!(snapshot(root), pristine);
        mods.set_enabled("adds", true).unwrap();
        mods.apply(&game_now, ModApplyOptions::default()).unwrap();
        mods.uninstall("adds").unwrap();
        let game_now = GameInstance::open(root).unwrap();
        mods.apply(&game_now, ModApplyOptions::default()).unwrap();
        assert_eq!(snapshot(root), pristine);
        assert!(!mods.has_written());

        // Exported again, the installed mod is the same package content.
        let mut mods = ModManager::open(&game).unwrap();
        mods.install_file(&game, &path, true).unwrap();
        let exported = temp.path().join("exported.zip");
        mods.export("adds", &exported).unwrap();
        let archive = ModArchive::open(&exported).unwrap();
        assert_eq!(
            *archive.manifest(),
            ModArchive::open(&path).unwrap().manifest().clone()
        );
    }
}

#[test]
fn clashes_between_adding_mods_are_found_and_stop_applying() {
    let (dir, game) = fake_game();
    let root = dir.path();
    let pristine = snapshot(root);
    let temp = tempfile::tempdir().unwrap();
    let t = temp.path();
    let a = build_adding_mod(
        &game,
        t,
        "a",
        vec![new_stream_def(PROPS, 0x0777_0000, false)],
        vec![
            added(PROPS, b"mesh", 0x1111, &[1u8; 100]),
            added(LEVEL, b"lua", 0x2222, b"lua of a"),
        ],
    );
    // The same stream, defined the same way (another label), with another resource.
    let mut same = new_stream_def(PROPS, 0x0777_0000, false);
    same.label = "Other label".into();
    let b = build_adding_mod(
        &game,
        t,
        "b",
        vec![same],
        vec![added(PROPS, b"mesh", 0x3333, &[3u8; 100])],
    );
    let c = build_adding_mod(
        &game,
        t,
        "c",
        vec![new_stream_def(PROPS, 0x0888_0000, true)],
        vec![],
    );
    let d = build_adding_mod(
        &game,
        t,
        "d",
        vec![new_stream_def("gf2/doctor/other", 0x0777_0000, false)],
        vec![],
    );
    let e = build_adding_mod(
        &game,
        t,
        "e",
        vec![],
        vec![added(TEXT, b"lua", 0x2222, b"other")],
    );
    let e_same = build_adding_mod(
        &game,
        t,
        "e-same",
        vec![],
        vec![added(TEXT, b"lua", 0x2222, b"lua of a")],
    );
    let f = build_mod(&game, t, "f", vec![(MESH, mesh(4))]);

    let mut mods = ModManager::open(&game).unwrap();
    for path in [&a, &b, &c, &d, &e, &e_same, &f] {
        mods.install_file(&game, path, false).unwrap();
    }
    let ids = |ids: &[&str], mods: &mut ModManager| {
        for m in ["a", "b", "c", "d", "e", "e-same", "f"] {
            mods.set_enabled(m, ids.contains(&m)).unwrap();
        }
    };
    let fatal = |mods: &ModManager| -> Vec<String> {
        mods.clashes(true)
            .iter()
            .filter(|c| c.is_fatal())
            .map(Clash::describe)
            .collect()
    };

    // a and b: the same stream defined alike holds the resources of both.
    ids(&["a", "b"], &mut mods);
    let clashes = mods.clashes(true);
    assert!(
        matches!(
            &clashes[..],
            [Clash::Stream {
                same_definition: true,
                ..
            }]
        ),
        "{clashes:?}"
    );
    mods.apply(&game, ModApplyOptions::default()).unwrap();
    let created = fs::read(root.join("gf2/doctor/props.str")).unwrap();
    let layout = crate::container::ustream::StreamLayout::parse(&created).unwrap();
    let ids_in: Vec<[u32; 4]> = layout.resources.iter().map(|r| r.header.ids).collect();
    assert_eq!(ids_in, [[0x1111; 4], [0x3333; 4]]);

    // a and c: the same name, another hash and parenting.
    ids(&["a", "c"], &mut mods);
    assert_eq!(fatal(&mods).len(), 1, "{:?}", mods.clashes(true));
    assert!(fatal(&mods)[0].contains("created differently"));
    let error = mods.apply(&game, ModApplyOptions::default()).unwrap_err();
    assert!(
        error.to_string().contains("cannot be applied together"),
        "{error}"
    );
    // a and d: two streams, one hash.
    ids(&["a", "d"], &mut mods);
    assert!(fatal(&mods)[0].contains("same hash"), "{:?}", fatal(&mods));
    assert!(mods.apply(&game, ModApplyOptions::default()).is_err());
    // a and e: one GUID added with other data; with the same data it is shared.
    ids(&["a", "e"], &mut mods);
    assert!(
        fatal(&mods)[0].contains("different data"),
        "{:?}",
        fatal(&mods)
    );
    assert!(mods.apply(&game, ModApplyOptions::default()).is_err());
    ids(&["a", "e-same"], &mut mods);
    assert!(fatal(&mods).is_empty());
    assert!(matches!(
        &mods.clashes(true)[..],
        [Clash::Guid {
            same_data: true,
            ..
        }]
    ));
    mods.apply(&game, ModApplyOptions::default()).unwrap();
    // a adds to the level, f replaces the mesh of the level: both are written.
    ids(&["a", "f"], &mut mods);
    let clashes = mods.clashes(true);
    assert!(
        matches!(&clashes[..], [Clash::SharedStream { stream, adding, replacing }]
            if stream == LEVEL && adding == &["a"] && replacing == &["f"]),
        "{clashes:?}"
    );
    assert!(!clashes[0].is_fatal());
    mods.apply(&game, ModApplyOptions::default()).unwrap();
    let level = game.open_stream(stream(&game, LEVEL)).unwrap();
    assert_eq!(level.resource_data(MESH).unwrap(), mesh(4));
    assert!(
        level
            .layout()
            .resources
            .iter()
            .any(|r| r.header.ids == [0x2222; 4])
    );
    drop(level);
    // All clashes of the installed mods, enabled or not.
    assert!(mods.clashes(false).len() >= 4);

    ids(&[], &mut mods);
    mods.apply(&game, ModApplyOptions::default()).unwrap();
    assert_eq!(snapshot(root), pristine);
}

#[test]
fn adding_mods_that_do_not_fit_the_game_are_refused() {
    let (dir, game) = fake_game();
    let root = dir.path();
    let pristine = snapshot(root);
    let temp = tempfile::tempdir().unwrap();
    let t = temp.path();
    let creator = build_adding_mod(
        &game,
        t,
        "creator",
        vec![new_stream_def(PROPS, 0x0777_0000, false)],
        vec![added(PROPS, b"mesh", 0x1111, &[1u8; 100])],
    );
    let base = manifest_from_workspace(
        &adding_workspace(
            &game,
            &t.join("ws-base"),
            vec![new_stream_def("gf2/doctor/x", 0x0999_0000, false)],
            vec![added(LEVEL, b"lua", 0x4444, b"lua")],
        ),
        &info("misfit"),
        None,
    )
    .unwrap();
    let lua = [b"lua".to_vec()];
    let mut mods = ModManager::open(&game).unwrap();
    mods.install_file(&game, &creator, true).unwrap();
    let mut install = |manifest: ModManifest, data: &[Vec<u8>]| -> Vec<Problem> {
        let path = t.join(format!("{}.zip", manifest.id));
        write_mod(&path, &manifest, data);
        let report = mods.install_file(&game, &path, true).unwrap();
        let problems = report.issues.into_iter().map(|i| i.problem).collect();
        mods.uninstall(&manifest.id).unwrap();
        problems
    };
    assert_eq!(install(base.clone(), &lua), []);

    // The added resource is already in the stream.
    let mut m = base.clone();
    m.resources[0].key.ids = [3; 4];
    m.resources[0].key.type_id = crate::hash::sdbm(b"mesh");
    assert_eq!(install(m, &lua), [Problem::ResourceExists]);
    // Its GUID belongs to the string table of the level, with other data.
    let mut m = base.clone();
    m.resources[0].key.ids = [9; 4];
    let problems = install(m.clone(), &lua);
    assert!(
        matches!(&problems[..], [Problem::GuidInUse { other }] if other == &key(&game, LEVEL, STRINGS)),
        "{problems:?}"
    );
    // With the same data the game shares the resource: fine.
    let table = game_data(&game, LEVEL, STRINGS);
    m.resources[0].key.stream = TEXT.into();
    m.resources[0].payload = Blob::of(&table);
    assert_eq!(install(m, &[table]), []);
    // Placed before a resource that is not there: added at the end.
    let mut m = base.clone();
    let missing = ResourceKey {
        ids: [0x77; 4],
        ..key(&game, LEVEL, MESH)
    };
    m.resources[0].added.as_mut().unwrap().before = Some(missing.clone());
    assert_eq!(
        install(m, &lua),
        [Problem::AnchorMissing { before: missing }]
    );
    // An added resource in a stream that is neither in the game nor new.
    let mut m = base.clone();
    m.resources[0].key.stream = "gf2/nowhere".into();
    assert_eq!(install(m, &lua), [Problem::StreamMissing]);
    // A replacement in the stream another mod creates.
    let mut m = base.clone();
    m.resources[0].key.stream = PROPS.into();
    m.resources[0].added = None;
    m.resources[0].original = Blob::of(b"old");
    assert_eq!(
        install(m, &lua),
        [Problem::InNewStream {
            creator: "creator".into()
        }]
    );
    // New streams: a name of the game, a hash of the table, a missing table or parent.
    let mut m = base.clone();
    m.streams[0].name = TEXT.into();
    assert_eq!(install(m, &lua), [Problem::StreamExists]);
    let mut m = base.clone();
    m.streams[0].hash = 0x100;
    assert_eq!(
        install(m, &lua),
        [Problem::StreamHashUsed {
            toc: "gf2/level.toc".into()
        }]
    );
    let mut m = base.clone();
    m.streams[0].tocs = vec!["gf2/missing.toc".into()];
    assert_eq!(
        install(m, &lua),
        [Problem::TocMissing {
            toc: "gf2/missing.toc".into()
        }]
    );
    let mut m = base.clone();
    m.streams[0].parent = "gf2/nothere".into();
    assert_eq!(
        install(m, &lua),
        [Problem::ParentMissing {
            toc: "gf2/level.toc".into(),
            parent: "gf2/nothere".into()
        }]
    );

    // A fatal misfit stops applying even with --ignore-compatibility; a
    // missing anchor does not stop it.
    let mut m = base.clone();
    m.resources[0].key.ids = [9; 4];
    let path = t.join("guid.zip");
    write_mod(&path, &m, &lua);
    mods.install_file(&game, &path, true).unwrap();
    let error = mods
        .apply(
            &game,
            ModApplyOptions {
                ignore_compatibility: true,
                force: false,
            },
        )
        .unwrap_err();
    assert!(error.to_string().contains("already used by"), "{error}");
    mods.uninstall("misfit").unwrap();
    let mut m = base.clone();
    m.resources[0].added.as_mut().unwrap().before = Some(ResourceKey {
        ids: [0x77; 4],
        ..key(&game, LEVEL, MESH)
    });
    let path = t.join("anchor.zip");
    write_mod(&path, &m, &lua);
    mods.install_file(&game, &path, true).unwrap();
    let report = mods.apply(&game, ModApplyOptions::default()).unwrap();
    assert!(
        report
            .warnings
            .iter()
            .any(|w| w.contains("added at the end")),
        "{:?}",
        report.warnings
    );
    let level = game.open_stream(stream(&game, LEVEL)).unwrap();
    let last = level.layout().resources.len() - 1;
    assert_eq!(level.layout().resources[last].header.ids, [0x4444; 4]);
    drop(level);
    mods.unapply(&game, false).unwrap();
    assert_eq!(snapshot(root), pristine);
}

#[test]
fn malicious_version_2_manifests_are_refused() {
    let (_dir, game) = fake_game();
    let temp = tempfile::tempdir().unwrap();
    let mut first = added(LEVEL, b"metamodel", 0x1234, b"m");
    first.before = Some(key(&game, LEVEL, MESH));
    let ws = adding_workspace(
        &game,
        &temp.path().join("ws"),
        vec![new_stream_def(PROPS, 0x0777_0000, true)],
        vec![first, added(PROPS, b"mesh", 0x5678, &[5u8; 64])],
    );
    let good = manifest_from_workspace(&ws, &info("good"), None).unwrap();
    good.validate().unwrap();
    let level_added = good
        .resources
        .iter()
        .position(|r| r.key.stream == LEVEL)
        .unwrap();
    let props_added = 1 - level_added;

    let mut cases: Vec<(&str, ModManifest)> = Vec::new();
    for name in [
        "../evil",
        "gf2/../../etc/x",
        "/abs/x",
        "gf2\\x",
        "gf2/X",
        "gf2//x",
        "gf2/./x",
        "_doctor_toolkit/mods/x",
        "_toolkit/backups/x",
        "gf2/con",
        "gf2/aux.x",
        "gf2/x.",
        "c:/x",
        "gf2/x y",
        "",
    ] {
        let mut m = good.clone();
        m.streams[0].name = name.into();
        m.resources[props_added].key.stream = name.into();
        cases.push(("new stream name", m));
    }
    for toc in [
        "../x.toc",
        "gf2/level.str",
        "/gf2/level.toc",
        "gf2\\level.toc",
        "",
    ] {
        let mut m = good.clone();
        m.streams[0].tocs = vec![toc.into()];
        cases.push(("table of contents", m));
    }
    let mut m = good.clone();
    m.streams[0].tocs = vec!["gf2/level.toc".into(); 2];
    cases.push(("repeated", m));
    let mut m = good.clone();
    m.streams[0].tocs = (0..65).map(|i| format!("gf2/t{i}.toc")).collect();
    cases.push(("1 to 64", m));
    let mut m = good.clone();
    m.streams[0].tocs.clear();
    cases.push(("1 to 64", m));
    let mut m = good.clone();
    m.streams[0].hash = 0;
    cases.push(("zero or repeated hash", m));
    let mut m = good.clone();
    let mut twin = m.streams[0].clone();
    twin.name = "gf2/doctor/twin".into();
    m.streams.push(twin);
    cases.push(("zero or repeated hash", m));
    let mut m = good.clone();
    m.streams.push(m.streams[0].clone());
    cases.push(("listed twice", m));
    let mut m = good.clone();
    m.streams[0].guid = "zz".into();
    cases.push(("32 hexadecimal", m));
    let mut m = good.clone();
    m.streams[0].parent = "../x".into();
    cases.push(("bad parent", m));
    let mut m = good.clone();
    m.streams[0].parent = PROPS.into();
    cases.push(("bad parent", m));
    let mut m = good.clone();
    m.streams[0]
        .toc_parents
        .insert("gf2/other.toc".into(), LEVEL.into());
    cases.push(("bad parent", m));
    let mut m = good.clone();
    let mut child = m.streams[0].clone();
    child.name = "gf2/doctor/child".into();
    child.hash = 0x0777_0001;
    child.parent = PROPS.into();
    m.streams.push(child);
    cases.push(("another new stream", m));
    let mut m = good.clone();
    m.resources[level_added].original = Blob::of(b"x");
    cases.push(("`original` must be empty", m));
    for alignment in [0, 3, 0x2_0000] {
        let mut m = good.clone();
        m.resources[level_added].added.as_mut().unwrap().alignment = alignment;
        cases.push(("power of two", m));
    }
    let mut m = good.clone();
    m.resources[level_added].added.as_mut().unwrap().before = Some(ResourceKey {
        stream: TEXT.into(),
        ..key(&game, LEVEL, MESH)
    });
    cases.push(("bad place", m));
    let mut m = good.clone();
    m.resources[props_added].added.as_mut().unwrap().before = Some(key(&game, LEVEL, MESH));
    cases.push(("cannot be placed", m));
    let mut m = good.clone();
    m.resources[props_added].added = None;
    m.resources[props_added].original = Blob::of(b"x");
    cases.push(("only added resources", m));
    let mut m = good.clone();
    let mut copy = m.resources[props_added].clone();
    copy.key.type_id = crate::hash::sdbm(b"lua");
    copy.payload = Blob::of(b"other");
    m.resources.push(copy);
    cases.push(("same GUID with different data", m));
    let mut m = good.clone();
    m.streams = (0..=MAX_NEW_STREAMS)
        .map(|i| new_stream_def(&format!("gf2/s{i}"), i as u32 + 1, false))
        .collect();
    cases.push(("new streams; at most", m));
    let mut m = good.clone();
    m.streams[0].label = "a\u{1b}[31m".into();
    cases.push(("control", m));
    for (expected, manifest) in cases {
        let error = manifest.validate().unwrap_err().to_string();
        assert!(error.contains(expected), "{expected}: {error}");
        // Parsing refuses the same JSON.
        let json = manifest.to_json().unwrap();
        assert!(ModManifest::parse(&json).is_err(), "{expected}");
    }
    // Unknown fields of a later minor change are ignored, as in version 1.
    let mut json: serde_json::Value = serde_json::from_slice(&good.to_json().unwrap()).unwrap();
    json["streams"][0]["future"] = 1.into();
    ModManifest::parse(json.to_string().as_bytes()).unwrap();
}

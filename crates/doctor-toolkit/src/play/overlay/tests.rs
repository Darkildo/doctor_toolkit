use std::collections::BTreeMap;
use std::fs;

use super::*;
use crate::workspace::FileKey;
use crate::workspace::tests::{
    LEVEL, MESH, edit, fake_game_with_files, new_resource, new_stream, stream,
};

#[test]
fn modes_and_paths() {
    assert_eq!(OverlayMode::default(), OverlayMode::Auto);
    assert_eq!(serde_json::to_string(&OverlayMode::Off).unwrap(), "\"off\"");
    if !cfg!(windows) {
        assert_eq!(
            game_path(Path::new("/srv/cache/x/gf2/cuba.str")).as_deref(),
            Some("Z:\\srv\\cache\\x\\gf2\\cuba.str")
        );
        assert_eq!(game_path(Path::new("relative")), None);
    }
    let r = redirect("GF2/Cuba.str", "Z:\\a\\b".into()).unwrap();
    assert_eq!(r.from, "gf2/cuba.str");
    assert!(redirect("gf2/a#b.str", "x".into()).is_err());
    assert!(redirect("../x", "x".into()).is_err());
}

/// Every file under `root`, the toolkit's folder too, with its contents.
fn everything(root: &Path) -> BTreeMap<String, Vec<u8>> {
    fn walk(dir: &Path, rel: &str, out: &mut BTreeMap<String, Vec<u8>>) {
        for entry in fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            let name = entry.file_name().into_string().unwrap();
            let rel = if rel.is_empty() {
                name
            } else {
                format!("{rel}/{name}")
            };
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

fn copy_dir(from: &Path, to: &Path) {
    for (rel, data) in everything(from) {
        let path = to.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, data).unwrap();
    }
}

/// The file the game gets for `rel` with the module's redirects.
fn seen_by_game(root: &Path, redirects: &[Redirect], rel: &str) -> Option<Vec<u8>> {
    let key = doctor_toolkit_live::files::key(rel).unwrap();
    match redirects.iter().find(|r| r.from == key) {
        Some(r) if r.relative() => fs::read(root.join(&r.to)).ok(),
        Some(r) => {
            let path = if cfg!(windows) {
                r.to.clone()
            } else {
                r.to.trim_start_matches("Z:").replace('\\', "/")
            };
            fs::read(path).ok()
        }
        None => fs::read(root.join(rel)).ok(),
    }
}

#[test]
fn the_game_sees_what_applying_would_write_and_its_folder_stays() {
    let (dir, game) = fake_game_with_files();
    let root = dir.path();
    let mut ws = Workspace::for_game(&game, None).unwrap();
    // A replaced resource, an archive entry, a movie and a new stream.
    let level = stream(&game, LEVEL);
    ws.set_resources(&game, "mesh", level, &[edit(MESH, vec![1; 10])])
        .unwrap();
    ws.set_file(
        &game,
        &FileKey::archive_entry("audiostreams/a/two.exa.snu"),
        vec![9; 3000],
        "sound",
    )
    .unwrap();
    ws.set_file(
        &game,
        &FileKey::loose("movies/common/mainmenu.vp6"),
        b"MVhd new movie".to_vec(),
        "movie",
    )
    .unwrap();
    let name = "gf2/doctor/props";
    ws.add_stream(&game, new_stream(name, false)).unwrap();
    ws.add_resources(
        &game,
        "new",
        &[new_resource(name, b"mesh", 0xBB, &[5u8; 1000])],
    )
    .unwrap();
    let before = everything(root);

    // The old way, on a copy: written into the game.
    let written = tempfile::tempdir().unwrap();
    copy_dir(root, written.path());
    let copy = GameInstance::open(written.path()).unwrap();
    Workspace::open(Workspace::default_dir(written.path()))
        .unwrap()
        .apply(&copy, ApplyOptions::default())
        .unwrap();

    let cache = tempfile::tempdir().unwrap();
    let overlay = Overlay::build(
        &game,
        Some(&Source::Workspace(Workspace::default_dir(root))),
        Some("-stream gf2 level\r\n"),
        &cache.path().join("overlay"),
    )
    .unwrap();
    // Nothing in the game folder changed, the toolkit's folder neither.
    assert_eq!(everything(root), before);
    let froms: Vec<&str> = overlay
        .redirects()
        .iter()
        .map(|r| r.from.as_str())
        .collect();
    for wanted in [
        "gf2/level.str",
        "gf2/level.toc",
        "gf2/doctor/props.str",
        "godfath0.viv",
        "godfath1.viv",
        "godfath2.viv",
        "movies/common/mainmenu.vp6",
        "cmd.txt",
    ] {
        assert!(froms.contains(&wanted), "{wanted} in {froms:?}");
    }
    assert!(
        !froms.contains(&"gf2/text/abcd.str"),
        "unchanged: {froms:?}"
    );
    // The archives one number up are the game's own files.
    let shifted = overlay
        .redirects()
        .iter()
        .find(|r| r.from == "godfath1.viv")
        .unwrap();
    assert_eq!(shifted.to, "GODFATH0.viv");
    // The game sees, file by file, what the old way wrote.
    for (rel, data) in everything(written.path()) {
        if rel.starts_with("_doctor_toolkit") {
            continue;
        }
        assert_eq!(
            seen_by_game(root, overlay.redirects(), &rel).as_ref(),
            Some(&data),
            "{rel}"
        );
    }
    assert_eq!(
        seen_by_game(root, overlay.redirects(), "cmd.txt").unwrap(),
        b"-stream gf2 level\r\n"
    );
    // The configuration of the module carries them.
    let config = doctor_toolkit_live::config::Config {
        redirects: overlay.redirects().to_vec(),
        ..Default::default()
    };
    assert_eq!(
        doctor_toolkit_live::config::Config::parse(&config.to_text()).redirects,
        overlay.redirects()
    );
    let staged = overlay.dir().to_path_buf();
    assert!(staged.is_dir());
    overlay.remove().unwrap();
    assert!(!staged.exists());
    assert_eq!(everything(root), before);
}

#[test]
fn without_changes_only_the_launch_options_are_redirected() {
    let (dir, game) = fake_game_with_files();
    let cache = tempfile::tempdir().unwrap();
    let overlay = Overlay::build(&game, None, Some("-windowed"), &cache.path().join("o")).unwrap();
    assert_eq!(overlay.redirects().len(), 1);
    assert_eq!(overlay.redirects()[0].from, "cmd.txt");
    let overlay = Overlay::build(
        &game,
        Some(&Source::Workspace(Workspace::default_dir(dir.path()))),
        None,
        &cache.path().join("o"),
    )
    .unwrap();
    assert!(overlay.redirects().is_empty());
    // Dropped: removed.
    let staged = overlay.dir().to_path_buf();
    drop(overlay);
    assert!(!staged.exists());
}

/// A mod of format 2 (a replaced mesh and a new stream) installed in the
/// game's mod manager: the game sees what `mods apply` would write, its
/// folder stays as it is.
#[test]
fn enabled_mods_of_format_2_are_staged() {
    let (dir, game) = fake_game_with_files();
    let root = dir.path();
    let temp = tempfile::tempdir().unwrap();
    let mut ws = Workspace::create(temp.path().join("ws"), &game).unwrap();
    ws.set_resources(
        &game,
        "mesh",
        stream(&game, LEVEL),
        &[edit(MESH, vec![2; 64])],
    )
    .unwrap();
    let name = "gf2/doctor/props";
    ws.add_stream(&game, new_stream(name, true)).unwrap();
    ws.add_resources(
        &game,
        "new",
        &[new_resource(name, b"mesh", 0xBB, &[5u8; 1000])],
    )
    .unwrap();
    let package = temp.path().join("props.zip");
    let info = crate::mods::ModInfo {
        id: "props".into(),
        name: "Props".into(),
        version: "1".into(),
        author: "tester".into(),
        description: String::new(),
        url: None,
        relations: Default::default(),
    };
    crate::mods::build_from_workspace(&ws, &info, None, None, &package).unwrap();
    let mut mods = ModManager::open(&game).unwrap();
    mods.install_file(&game, &package, true).unwrap();
    let before = everything(root);

    let written = tempfile::tempdir().unwrap();
    copy_dir(root, written.path());
    let copy = GameInstance::open(written.path()).unwrap();
    ModManager::open(&copy)
        .unwrap()
        .apply(&copy, ModApplyOptions::default())
        .unwrap();

    let overlay = Overlay::build(&game, Some(&Source::Mods), None, &temp.path().join("o")).unwrap();
    assert_eq!(everything(root), before);
    let mut froms: Vec<&str> = overlay
        .redirects()
        .iter()
        .map(|r| r.from.as_str())
        .collect();
    froms.sort();
    assert_eq!(
        froms,
        ["gf2/doctor/props.str", "gf2/level.str", "gf2/level.toc"]
    );
    for (rel, data) in everything(written.path()) {
        if !rel.starts_with("_doctor_toolkit") {
            assert_eq!(
                seen_by_game(root, overlay.redirects(), &rel).as_ref(),
                Some(&data),
                "{rel}"
            );
        }
    }
    // The mod manager of the game still has nothing in the game.
    assert!(!ModManager::open(&game).unwrap().has_written());
}

/// A path of the module's configuration as this computer sees it.
fn host_path(root: &Path, to: &str) -> std::path::PathBuf {
    if !doctor_toolkit_live::files::is_absolute(to) {
        root.join(to)
    } else if cfg!(windows) {
        to.into()
    } else {
        to.trim_start_matches("Z:").replace('\\', "/").into()
    }
}

/// What the game reads for `rel` when the module composes the patched
/// streams in memory (stage 100) and redirects the rest.
fn read_by_game(root: &Path, overlay: &Overlay, rel: &str) -> Option<Vec<u8>> {
    use doctor_toolkit_live::resources::compose_files;
    let key = doctor_toolkit_live::files::key(rel).unwrap();
    let Some(patch) = overlay.resources().iter().find(|r| r.from == key) else {
        return seen_by_game(root, overlay.redirects(), rel);
    };
    // The module composes from the game's own stream, or from the file
    // the line names.
    let original = match &patch.base {
        Some(base) => fs::read(host_path(root, base)).ok()?,
        None => fs::read(root.join(rel)).ok()?,
    };
    let patch = fs::read(host_path(root, &patch.to)).ok()?;
    let (mut o, mut p) = (&original[..], &patch[..]);
    let (_, _, composed) = compose_files(&mut o, &mut p).unwrap();
    let (mut o, mut p) = (&original[..], &patch[..]);
    Some(composed.to_vec(&mut o, &mut p).unwrap())
}

/// The resources of a stream file: header and data, in order.
fn resources_of(data: &[u8]) -> Vec<(crate::container::ustream::ResourceHeader, Vec<u8>)> {
    let layout = crate::container::ustream::StreamLayout::parse(data).unwrap();
    (0..layout.resources.len())
        .map(|i| {
            (
                layout.resources[i].header,
                layout.resource_data(data, i).unwrap(),
            )
        })
        .collect()
}

/// Stage 100: with [`Streams::Patched`] a changed stream is staged as a
/// resource patch. What the game reads then — the stream the module
/// composes from the game's file and the patch — holds, resource by
/// resource, what "apply" writes (same headers, same data, same order);
/// tables of contents, the created stream and the other files are the same
/// files; a stream whose changed resources "apply" stores raw is the same
/// file byte for byte. The game folder stays as it is and no copy of a
/// stream is made.
#[test]
fn patched_streams_hold_the_resources_applying_would_write() {
    use crate::workspace::tests::{STRINGS, TEXT, sample_table};
    let (dir, game) = fake_game_with_files();
    let root = dir.path();
    let mut ws = Workspace::for_game(&game, None).unwrap();
    // A compressed resource replaced by a larger one, a raw one replaced, a
    // resource added before another and one at the end; another stream
    // with a raw replacement only; a created stream.
    let level = stream(&game, LEVEL);
    ws.set_resources(
        &game,
        "mesh and strings",
        level,
        &[
            edit(MESH, (0..5000u32).map(|i| (i * 7) as u8).collect()),
            edit(
                STRINGS,
                sample_table(["Bonjour", "Monde"]).to_bytes().unwrap(),
            ),
        ],
    )
    .unwrap();
    let mut before_mesh = new_resource(LEVEL, b"mesh", 0xA1, &[3u8; 700]);
    before_mesh.before = Some(crate::workspace::tests::key(&game, LEVEL, MESH));
    ws.add_resources(
        &game,
        "added",
        &[before_mesh, new_resource(LEVEL, b"mesh", 0xA2, &[4u8; 33])],
    )
    .unwrap();
    ws.set_resources(
        &game,
        "text",
        stream(&game, TEXT),
        &[edit(0, sample_table(["Oui", "Non"]).to_bytes().unwrap())],
    )
    .unwrap();
    // Files outside the streams (a mod of format 4: an entry of the `.viv`
    // archives, a movie) go the way of stage 46.
    ws.set_file(
        &game,
        &FileKey::archive_entry("audiostreams/a/two.exa.snu"),
        vec![9; 3000],
        "sound",
    )
    .unwrap();
    ws.set_file(
        &game,
        &FileKey::loose("movies/common/mainmenu.vp6"),
        b"MVhd new movie".to_vec(),
        "movie",
    )
    .unwrap();
    let name = "gf2/doctor/props";
    ws.add_stream(&game, new_stream(name, false)).unwrap();
    ws.add_resources(
        &game,
        "new",
        &[new_resource(name, b"mesh", 0xBB, &[5u8; 1000])],
    )
    .unwrap();
    let before = everything(root);

    // The old way, on a copy: written into the game.
    let written = tempfile::tempdir().unwrap();
    copy_dir(root, written.path());
    let copy = GameInstance::open(written.path()).unwrap();
    Workspace::open(Workspace::default_dir(written.path()))
        .unwrap()
        .apply(&copy, ApplyOptions::default())
        .unwrap();

    let cache = tempfile::tempdir().unwrap();
    let overlay = Overlay::build_with(
        &game,
        Some(&Source::Workspace(Workspace::default_dir(root))),
        None,
        &cache.path().join("overlay"),
        Streams::Patched,
    )
    .unwrap();
    assert_eq!(everything(root), before);
    let mut patched: Vec<&str> = overlay
        .resources()
        .iter()
        .map(|r| r.from.as_str())
        .collect();
    patched.sort();
    assert_eq!(patched, ["gf2/level.str", "gf2/text/abcd.str"]);
    let mut redirected: Vec<&str> = overlay
        .redirects()
        .iter()
        .map(|r| r.from.as_str())
        .collect();
    redirected.sort();
    // The created stream is a file of its own; the table of contents is
    // the one "apply" writes (the data sizes of the streams in it); the
    // overlay archive and the movie are redirected as with stage 46.
    assert_eq!(
        redirected,
        [
            "gf2/doctor/props.str",
            "gf2/level.toc",
            "godfath0.viv",
            "godfath1.viv",
            "godfath2.viv",
            "movies/common/mainmenu.vp6"
        ]
    );
    for (rel, data) in everything(written.path()) {
        if rel.starts_with("_doctor_toolkit") {
            continue;
        }
        let read = read_by_game(root, &overlay, &rel).unwrap_or_else(|| panic!("{rel}"));
        if rel.ends_with(".str") {
            assert!(resources_of(&read) == resources_of(&data), "{rel}");
        } else {
            assert!(read == data, "{rel}");
        }
    }
    // The text stream's only change is stored raw by "apply": the same file.
    assert_eq!(
        read_by_game(root, &overlay, "gf2/text/abcd.str").unwrap(),
        fs::read(written.path().join("gf2/text/abcd.str")).unwrap()
    );
    // The patches are small files, not streams, and nothing of the staged
    // copy is a copy of a stream of the game: the "backup" of a patched
    // stream is a link to the game's file.
    for r in overlay.resources() {
        assert_eq!(r.base, None);
        let patch = fs::read(host_path(root, &r.to)).unwrap();
        assert!(doctor_toolkit_live::resources::Patch::is_patch(&patch));
        let backup = overlay
            .dir()
            .join("game")
            .join(crate::game::BACKUP_DIR)
            .join(&r.from);
        if cfg!(unix) {
            assert!(
                fs::symlink_metadata(&backup)
                    .unwrap()
                    .file_type()
                    .is_symlink(),
                "{}",
                backup.display()
            );
        }
    }
    // The configuration of the module carries both lists.
    let mut config = doctor_toolkit_live::config::Config::default();
    overlay.configure(&mut config);
    let back = doctor_toolkit_live::config::Config::parse(&config.to_text());
    assert_eq!(back.redirects, overlay.redirects());
    assert_eq!(back.resources, overlay.resources());
    overlay.remove().unwrap();
    assert_eq!(everything(root), before);
}

/// Stage 100 with the mod manager: the enabled mods as resource patches.
#[test]
fn enabled_mods_are_staged_as_resource_patches() {
    let (dir, game) = fake_game_with_files();
    let root = dir.path();
    let temp = tempfile::tempdir().unwrap();
    let mut ws = Workspace::create(temp.path().join("ws"), &game).unwrap();
    ws.set_resources(
        &game,
        "mesh",
        stream(&game, LEVEL),
        &[edit(MESH, vec![2; 64])],
    )
    .unwrap();
    let name = "gf2/doctor/props";
    ws.add_stream(&game, new_stream(name, true)).unwrap();
    ws.add_resources(
        &game,
        "new",
        &[new_resource(name, b"mesh", 0xBB, &[5u8; 1000])],
    )
    .unwrap();
    let package = temp.path().join("props.zip");
    let info = crate::mods::ModInfo {
        id: "props".into(),
        name: "Props".into(),
        version: "1".into(),
        author: "tester".into(),
        description: String::new(),
        url: None,
        relations: Default::default(),
    };
    crate::mods::build_from_workspace(&ws, &info, None, None, &package).unwrap();
    let mut mods = ModManager::open(&game).unwrap();
    mods.install_file(&game, &package, true).unwrap();
    let before = everything(root);

    let written = tempfile::tempdir().unwrap();
    copy_dir(root, written.path());
    let copy = GameInstance::open(written.path()).unwrap();
    ModManager::open(&copy)
        .unwrap()
        .apply(&copy, ModApplyOptions::default())
        .unwrap();

    let overlay = Overlay::build_with(
        &game,
        Some(&Source::Mods),
        None,
        &temp.path().join("o"),
        Streams::Patched,
    )
    .unwrap();
    assert_eq!(everything(root), before);
    let patched: Vec<&str> = overlay
        .resources()
        .iter()
        .map(|r| r.from.as_str())
        .collect();
    assert_eq!(patched, ["gf2/level.str"]);
    for (rel, data) in everything(written.path()) {
        if rel.starts_with("_doctor_toolkit") {
            continue;
        }
        let read = read_by_game(root, &overlay, &rel).unwrap_or_else(|| panic!("{rel}"));
        if rel.ends_with(".str") {
            assert!(resources_of(&read) == resources_of(&data), "{rel}");
        } else {
            assert!(read == data, "{rel}");
        }
    }
    assert!(!ModManager::open(&game).unwrap().has_written());
}

/// Stage 100, a game the workspace was written into earlier: the stream in
/// the game folder is not the original any more, and the patch of the new
/// changes is made for the backup. The line of the module names that file,
/// and what the game reads — composed from the backup and the patch —
/// holds the resources "apply" would write now.
#[test]
fn a_patch_for_a_stream_written_earlier_names_its_original() {
    let (dir, game) = fake_game_with_files();
    let root = dir.path();
    let mut ws = Workspace::for_game(&game, None).unwrap();
    let level = stream(&game, LEVEL);
    ws.set_resources(&game, "first", level, &[edit(MESH, vec![1; 2000])])
        .unwrap();
    ws.apply(&game, ApplyOptions::default()).unwrap();
    let game = GameInstance::open(root).unwrap();
    // The changes move on: another mesh and an added resource.
    ws.set_resources(&game, "second", level, &[edit(MESH, vec![9; 77])])
        .unwrap();
    ws.add_resources(
        &game,
        "added",
        &[new_resource(LEVEL, b"mesh", 0xA7, &[6u8; 500])],
    )
    .unwrap();
    let before = everything(root);

    let written = tempfile::tempdir().unwrap();
    copy_dir(root, written.path());
    let copy = GameInstance::open(written.path()).unwrap();
    Workspace::open(Workspace::default_dir(written.path()))
        .unwrap()
        .apply(&copy, ApplyOptions::default())
        .unwrap();

    let cache = tempfile::tempdir().unwrap();
    let overlay = Overlay::build_with(
        &game,
        Some(&Source::Workspace(Workspace::default_dir(root))),
        None,
        &cache.path().join("overlay"),
        Streams::Patched,
    )
    .unwrap();
    assert_eq!(everything(root), before);
    assert_eq!(overlay.resources().len(), 1);
    let line = &overlay.resources()[0];
    assert_eq!(line.from, "gf2/level.str");
    let base = line.base.as_deref().expect("the backup is named");
    assert!(
        base.replace('\\', "/")
            .ends_with("_doctor_toolkit/backups/gf2/level.str"),
        "{base}"
    );
    // The backup is the original, not the stream written earlier.
    assert_ne!(
        fs::read(host_path(root, base)).unwrap(),
        fs::read(root.join("gf2/level.str")).unwrap()
    );
    let read = read_by_game(root, &overlay, "gf2/level.str").unwrap();
    let want = fs::read(written.path().join("gf2/level.str")).unwrap();
    assert!(resources_of(&read) == resources_of(&want));
    assert_eq!(
        read_by_game(root, &overlay, "gf2/level.toc").unwrap(),
        fs::read(written.path().join("gf2/level.toc")).unwrap()
    );
}

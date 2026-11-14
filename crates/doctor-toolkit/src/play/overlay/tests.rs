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

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
        whole_resources: false,
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
        whole_resources: false,
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

/// Every file below `dir`, links too.
fn everything_below(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if fs::symlink_metadata(&path).unwrap().is_dir() {
            out.extend(everything_below(&path));
        } else {
            out.push(path);
        }
    }
    out
}

/// The words of the string table that is resource `index` of the stream
/// file `data`.
fn words_of(data: &[u8], index: usize) -> Vec<String> {
    let (_, table) = resources_of(data).swap_remove(index);
    crate::resource::lh2::StringTable::parse(&table)
        .unwrap()
        .strings
        .swap_remove(0)
}

/// Sets word `row` of the string table that is resource `index` of
/// `stream`, as the editor does: in the table as `ws` has it, else as it is
/// in the game files (with the mods written there).
fn set_word(
    ws: &mut Workspace,
    game: &GameInstance,
    stream: &str,
    index: usize,
    row: usize,
    word: &str,
) {
    use crate::workspace::tests::{game_data, key};
    let shown = ws
        .data(&key(game, stream, index))
        .unwrap()
        .unwrap_or_else(|| game_data(game, stream, index));
    let mut table = crate::resource::lh2::StringTable::parse(&shown).unwrap();
    table.strings[0][row] = word.into();
    let at = game.find_stream(stream).unwrap();
    ws.set_string_table(game, at, index, &table).unwrap();
}

/// Stage 107: a project is staged as the last mod of the game's mod
/// manager. With a mod written into the game files — where the project's
/// workspace by itself is refused — the game reads both: the mod's word
/// and the project's word in the table both change, and the project's own
/// table. The game folder, the registry of the mod manager and its record
/// of what is written stay byte for byte; so does the project.
#[test]
fn a_project_is_staged_on_top_of_the_mods_written_into_the_game() {
    use crate::project::{NewDocument, create_document};
    use crate::testing::{LEVEL, STRINGS, TEXT, write_game, write_town};
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("game");
    fs::create_dir_all(&root).unwrap();
    write_game(&root).unwrap();
    write_town(&root).unwrap();
    let game = GameInstance::open(&root).unwrap();

    // The base mod, made in a workspace of its own and written into the game.
    let mut base = Workspace::for_game(&game, Some(&temp.path().join("base"))).unwrap();
    set_word(&mut base, &game, TEXT, 0, 0, "Base");
    let package = temp.path().join("base.zip");
    let info = crate::mods::ModInfo {
        id: "base".into(),
        name: "Base".into(),
        version: "1.2".into(),
        ..Default::default()
    };
    crate::mods::build_from_workspace(&base, &info, None, None, &package).unwrap();
    let mut mods = ModManager::open(&game).unwrap();
    mods.install_file(&game, &package, true).unwrap();
    mods.apply(&game, ModApplyOptions::default()).unwrap();
    let game = GameInstance::open(&root).unwrap();

    // The project, edited on the game as it is with the mod.
    let at = temp.path().join("My Mod");
    let project = create_document(
        &game,
        &at,
        &NewDocument {
            name: "My Mod".into(),
            ..Default::default()
        },
    )
    .unwrap();
    let mut ws = project.workspace(&game).unwrap();
    set_word(&mut ws, &game, TEXT, 0, 1, "Mine");
    set_word(&mut ws, &game, LEVEL, STRINGS, 0, "Project");
    let ws_dir = at.join("workspace");
    assert_eq!(
        Source::of_workspace(ws_dir.clone()),
        Source::Project(at.clone())
    );
    assert_eq!(
        Source::of_workspace(Workspace::default_dir(&root)),
        Source::Workspace(Workspace::default_dir(&root))
    );
    let before = everything(&root);
    let project_before = everything(&at);

    // What installing the project and writing the mods would give, on a copy.
    let written = tempfile::tempdir().unwrap();
    copy_dir(&root, written.path());
    let copy = GameInstance::open(written.path()).unwrap();
    let mut copy_mods = ModManager::open(&copy).unwrap();
    project
        .install_into(&copy, &mut copy_mods, &ws, Default::default())
        .unwrap();
    copy_mods.apply(&copy, ModApplyOptions::default()).unwrap();
    assert_eq!(
        copy_mods
            .mods()
            .iter()
            .map(|m| m.id.as_str())
            .collect::<Vec<_>>(),
        ["base", "my-mod"]
    );
    // It was made on the mod's data: it needs the mod and wins as a patch.
    let installed = copy_mods.get("my-mod").unwrap();
    assert!(installed.enabled);
    assert_eq!(installed.manifest.version, "1.0");
    assert!(installed.manifest.depends.contains_key("base"));

    // The workspace by itself is not staged: a project's is not written,
    // and the mods are in the game.
    let alone = Overlay::build(
        &game,
        Some(&Source::Workspace(ws_dir.clone())),
        None,
        &temp.path().join("alone"),
    )
    .unwrap_err()
    .to_string();
    assert!(alone.contains("a project's own"), "{alone}");

    for streams in [Streams::Rebuilt, Streams::Patched] {
        let overlay = Overlay::build_with(
            &game,
            Some(&Source::Project(at.clone())),
            None,
            &temp.path().join("o"),
            streams,
        )
        .unwrap();
        // The project's data is not copied for the launch: the payloads
        // of its staged mod are links to the files of its workspace.
        let staged_mod = overlay.dir().join("game/_doctor_toolkit/mods/my-mod");
        assert!(staged_mod.join("mod.json").is_file());
        let payloads = everything_below(&staged_mod.join("payloads"));
        assert_eq!(payloads.len(), 2, "{payloads:?}");
        for payload in payloads {
            let link = fs::read_link(&payload).unwrap();
            assert!(link.starts_with(&ws_dir), "{}", link.display());
        }
        // Nothing of the game changed, the mod manager's files neither.
        assert!(
            everything(&root) == before,
            "{streams:?}: the game folder changed"
        );
        assert!(
            everything(&at) == project_before,
            "{streams:?}: the project changed"
        );
        let text = read_by_game(&root, &overlay, "gf2/text/abcd.str").unwrap();
        assert_eq!(words_of(&text, 0), ["Base", "Mine"], "{streams:?}");
        let level = read_by_game(&root, &overlay, "gf2/level.str").unwrap();
        assert_eq!(words_of(&level, STRINGS)[0], "Project", "{streams:?}");
        for (rel, data) in everything(written.path()) {
            if rel.starts_with("_doctor_toolkit") {
                continue;
            }
            let read = read_by_game(&root, &overlay, &rel).unwrap_or_else(|| panic!("{rel}"));
            if rel.ends_with(".str") {
                assert!(
                    resources_of(&read) == resources_of(&data),
                    "{streams:?}: {rel}"
                );
            } else {
                assert!(read == data, "{streams:?}: {rel}");
            }
        }
        overlay.remove().unwrap();
    }
    // The game's mod manager knows nothing of the project.
    let mods = ModManager::open(&game).unwrap();
    assert!(mods.get("my-mod").is_none());
    assert_eq!(mods.mods().len(), 1);

    // The mod alone, as before.
    let overlay = Overlay::build(&game, Some(&Source::Mods), None, &temp.path().join("o")).unwrap();
    assert!(
        overlay.redirects().is_empty(),
        "the mods are in the game already"
    );
    assert!(everything(&root) == before);
}

/// Stage 110: the staged mod manager folds the deltas of the mods. Two mods
/// that add a crate each to one SimGroup, enabled and not written into the
/// game: the game reads a yard with both. A project on top of them is
/// staged as a delta too, and the yard has all three. The game folder,
/// with the mod manager's files and its folds, stays byte for byte.
#[test]
fn the_staged_mods_and_a_project_are_folded() {
    use crate::mods::fold::tests::{add_crate, edit_group, guid, has, info, town, yard_mod};
    use crate::project::{NewDocument, create_document};
    use crate::resource::simgroup::SimGroup;
    use crate::testing::{YARD, YARD_SIMGROUP};
    use crate::workspace::tests::game_data;
    let (dir, game) = town();
    let root = dir.path();
    let temp = tempfile::tempdir().unwrap();
    let mut mods = ModManager::open(&game).unwrap();
    for (id, n) in [("a", 21), ("b", 22)] {
        let path = yard_mod(&game, temp.path(), &info(id, "1.0"), |g| add_crate(g, n));
        mods.install_file(&game, &path, true).unwrap();
    }
    let yard_of = |overlay: &Overlay| {
        let file = read_by_game(root, overlay, "gf2/town/yard.str").unwrap();
        let (_, data) = resources_of(&file).swap_remove(YARD_SIMGROUP);
        SimGroup::parse(&data).unwrap()
    };
    let before = everything(root);
    for streams in [Streams::Rebuilt, Streams::Patched] {
        let overlay = Overlay::build_with(
            &game,
            Some(&Source::Mods),
            None,
            &temp.path().join("o"),
            streams,
        )
        .unwrap();
        let yard = yard_of(&overlay);
        assert!(has(&yard, 21) && has(&yard, 22), "{streams:?}");
        assert_eq!(yard.objects.len(), 15, "{streams:?}");
        // The folds are the staged copy's: the game has none.
        assert!(
            overlay
                .dir()
                .join("game/_doctor_toolkit/mods/.folded/index.json")
                .is_file()
        );
        assert!(
            everything(root) == before,
            "{streams:?}: the game folder changed"
        );
        overlay.remove().unwrap();
    }

    // Written into the game, the mods leave their folds there; a staged
    // copy then links to them and computes nothing anew.
    mods.apply(&game, ModApplyOptions::default()).unwrap();
    let game = GameInstance::open(root).unwrap();
    let written = everything(root);
    let overlay = Overlay::build(&game, Some(&Source::Mods), None, &temp.path().join("o")).unwrap();
    assert!(
        overlay.redirects().is_empty(),
        "the mods are in the game already"
    );
    let index = overlay
        .dir()
        .join("game/_doctor_toolkit/mods/.folded/index.json");
    assert!(
        fs::symlink_metadata(&index)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    overlay.remove().unwrap();
    assert!(everything(root) == written);

    // A project made on the game with both mods in it.
    let at = temp.path().join("My Mod");
    let project = create_document(
        &game,
        &at,
        &NewDocument {
            name: "My Mod".into(),
            ..Default::default()
        },
    )
    .unwrap();
    let mut ws = project.workspace(&game).unwrap();
    let mine = edit_group(&game_data(&game, YARD, YARD_SIMGROUP), |g| add_crate(g, 60));
    let yard = game.find_stream(YARD).unwrap();
    ws.set_resources(&game, "crate", yard, &[edit(YARD_SIMGROUP, mine)])
        .unwrap();
    drop(ws);
    let project_before = everything(&at);
    for streams in [Streams::Rebuilt, Streams::Patched] {
        let overlay = Overlay::build_with(
            &game,
            Some(&Source::Project(at.clone())),
            None,
            &temp.path().join("o"),
            streams,
        )
        .unwrap();
        // The project is staged as a delta on what the two mods give.
        let staged = overlay
            .dir()
            .join("game/_doctor_toolkit/mods/my-mod/mod.json");
        let manifest = crate::mods::ModManifest::parse(&fs::read(staged).unwrap()).unwrap();
        assert_eq!(manifest.deltas().count(), 1, "{streams:?}");
        assert_eq!(
            manifest.depends.keys().collect::<Vec<_>>(),
            ["a", "b"],
            "{streams:?}"
        );
        let yard = yard_of(&overlay);
        assert!(
            has(&yard, 21) && has(&yard, 22) && has(&yard, 60),
            "{streams:?}"
        );
        assert_eq!(yard.objects.len(), 16, "{streams:?}");
        assert!(yard.objects[15].guid == guid(60), "{streams:?}");
        assert!(
            everything(root) == written,
            "{streams:?}: the game folder changed"
        );
        assert!(
            everything(&at) == project_before,
            "{streams:?}: the project changed"
        );
        overlay.remove().unwrap();
    }
}

/// A project that is installed into the game's mods and written is played
/// as it is now: the staged manager replaces the installed state. A project
/// without changes is no mod, and its installed state is switched off for
/// the launch. A replaced movie, which a mod does not carry, is staged as
/// a file. The game folder stays as it is every time.
#[test]
fn the_installed_state_of_a_project_is_replaced_for_the_launch() {
    use crate::project::{NewDocument, create_document};
    use crate::testing::{TEXT, write_game};
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("game");
    fs::create_dir_all(root.join("movies")).unwrap();
    write_game(&root).unwrap();
    fs::write(root.join("movies/intro.vp6"), b"MVhd old").unwrap();
    let game = GameInstance::open(&root).unwrap();
    let at = temp.path().join("p");
    let project = create_document(
        &game,
        &at,
        &NewDocument {
            name: "P".into(),
            ..Default::default()
        },
    )
    .unwrap();
    let build = |name: &str| {
        Overlay::build(
            &game,
            Some(&Source::Project(at.clone())),
            None,
            &temp.path().join(name),
        )
        .unwrap()
    };

    // Nothing edited yet, no mods: nothing is staged, no folder is made.
    let before = everything(&root);
    assert!(build("o").redirects().is_empty());
    assert!(everything(&root) == before);
    assert!(!root.join("_doctor_toolkit").exists());

    // Installed into Mods and written into the game.
    let mut ws = project.workspace(&game).unwrap();
    set_word(&mut ws, &game, TEXT, 0, 0, "One");
    let mut mods = ModManager::open(&game).unwrap();
    project
        .install_into(&game, &mut mods, &ws, Default::default())
        .unwrap();
    mods.apply(&game, ModApplyOptions::default()).unwrap();
    let game = GameInstance::open(&root).unwrap();
    assert_eq!(
        words_of(&fs::read(root.join("gf2/text/abcd.str")).unwrap(), 0)[0],
        "One"
    );

    // Edited further, and a movie replaced.
    let mut ws = project.workspace(&game).unwrap();
    set_word(&mut ws, &game, TEXT, 0, 1, "Two");
    ws.set_file(
        &game,
        &FileKey::loose("movies/intro.vp6"),
        b"MVhd new".to_vec(),
        "movie",
    )
    .unwrap();
    let before = everything(&root);
    let overlay = build("o");
    assert!(everything(&root) == before);
    let text = seen_by_game(&root, overlay.redirects(), "gf2/text/abcd.str").unwrap();
    assert_eq!(words_of(&text, 0), ["One", "Two"]);
    assert_eq!(
        seen_by_game(&root, overlay.redirects(), "movies/intro.vp6").unwrap(),
        b"MVhd new"
    );
    overlay.remove().unwrap();

    // Every edit taken back: the launch shows the game without the project,
    // though its installed state is still written in the game files.
    ws.revert_all().unwrap();
    let overlay = build("o");
    assert!(everything(&root) == before);
    let text = seen_by_game(&root, overlay.redirects(), "gf2/text/abcd.str").unwrap();
    assert_eq!(words_of(&text, 0), ["Yes", "No"]);
    assert!(ModManager::open(&game).unwrap().get("p").unwrap().enabled);

    // While another program changes the mods, they are not staged.
    let held = RegistryLock::acquire(&crate::mods::mods_dir(&root), "open-mod-manager").unwrap();
    let error = Overlay::build(
        &game,
        Some(&Source::Project(at.clone())),
        None,
        &temp.path().join("busy"),
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("changed by open-mod-manager"), "{error}");
    drop(held);
}

//! Detection on a fake home directory with every kind of launcher.

use std::fs;
use std::path::{Path, PathBuf};

use super::*;

/// A folder that passes [`looks_like_game`].
fn fake_game(dir: &Path) {
    fs::create_dir_all(dir.join("gf2")).unwrap();
    fs::write(dir.join("godfather2.exe"), b"MZ").unwrap();
    fs::write(dir.join("gf2/frontend.toc"), b"toc").unwrap();
}

fn write(path: &Path, text: impl AsRef<[u8]>) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

fn shortcuts_vdf(app_id: u32, exe: &str) -> Vec<u8> {
    let mut data = vec![0u8];
    data.extend_from_slice(b"shortcuts\0");
    data.push(0);
    data.extend_from_slice(b"0\0");
    data.push(2);
    data.extend_from_slice(b"appid\0");
    data.extend_from_slice(&app_id.to_le_bytes());
    data.push(1);
    data.extend_from_slice(b"AppName\0The Godfather II\0");
    data.push(1);
    data.extend_from_slice(format!("Exe\0\"{exe}\"\0").as_bytes());
    data.extend_from_slice(&[8, 8, 8]);
    data
}

struct Tree {
    _dir: tempfile::TempDir,
    root: PathBuf,
    roots: SearchRoots,
}

fn tree() -> Tree {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let home = root.join("home");
    let data = home.join(".local/share");
    let config = home.join(".config");

    // Native Steam with a second library holding the Steam game, a
    // Proton prefix of a shortcut and the shortcut itself.
    let steam = data.join("Steam");
    let library = root.join("library");
    write(
        &steam.join("steamapps/libraryfolders.vdf"),
        format!(
            "\"libraryfolders\"\n{{\n\t\"0\" {{ \"path\" \"{}\" }}\n\t\"1\" {{ \"path\" \"{}\" }}\n}}\n",
            steam.display(),
            library.display()
        ),
    );
    write(
        &library.join("steamapps/appmanifest_24830.acf"),
        "\"AppState\"\n{\n\t\"appid\" \"24830\"\n\t\"installdir\" \"The Godfather II\"\n}\n",
    );
    fake_game(&library.join("steamapps/common/The Godfather II"));
    let pfx = steam.join("steamapps/compatdata/2934567890/pfx");
    write(&pfx.join("system.reg"), "WINE REGISTRY Version 2\n");
    fake_game(&pfx.join("drive_c/Program Files (x86)/EA Games/The Godfather II"));
    let shortcut_game = root.join("elsewhere/GF2 copy");
    fake_game(&shortcut_game);
    write(
        &steam.join("userdata/123/config/shortcuts.vdf"),
        shortcuts_vdf(
            3_000_000_000,
            &shortcut_game.join("godfather2.exe").to_string_lossy(),
        ),
    );
    // The same Steam through ~/.steam/steam is not searched twice.
    fs::create_dir_all(home.join(".steam")).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&steam, home.join(".steam/steam")).unwrap();

    // The default Wine prefix: the registry names a folder on drive D:.
    let wine = home.join(".wine");
    let d_drive = root.join("d-drive");
    fake_game(&d_drive.join("Games/GF2"));
    write(
        &wine.join("system.reg"),
        "WINE REGISTRY Version 2\n\n\
         [Software\\\\Wow6432Node\\\\EA Games\\\\The Godfather II] 1700000000\n\
         \"DisplayName\"=\"The Godfather II\"\n\
         \"Install Dir\"=\"D:\\\\Games\\\\GF2\\\\\"\n",
    );
    fs::create_dir_all(wine.join("drive_c")).unwrap();
    fs::create_dir_all(wine.join("dosdevices")).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&d_drive, wine.join("dosdevices/d:")).unwrap();

    // Lutris: a game config naming the executable and its prefix.
    let lutris_prefix = home.join("Games/the-godfather-ii");
    fake_game(&lutris_prefix.join("drive_c/GF2"));
    write(
        &data.join("lutris/games/the-godfather-ii-1700000000.yml"),
        format!(
            "game:\n  exe: {}\n  prefix: {}\nwine:\n  version: lutris-7\n",
            lutris_prefix.join("drive_c/GF2/godfather2.exe").display(),
            lutris_prefix.display()
        ),
    );

    // Heroic: a sideloaded game.
    let heroic_game = root.join("heroic/Godfather");
    fake_game(&heroic_game);
    write(
        &config.join("heroic/sideload_apps/library.json"),
        serde_json::json!({
            "games": [{
                "app_name": "gf2sideload",
                "title": "The Godfather II",
                "folder_name": heroic_game,
                "install": { "executable": heroic_game.join("godfather2.exe") }
            }]
        })
        .to_string(),
    );

    // Bottles: a bottle with the game in its program folder.
    let bottle = data.join("bottles/bottles/Godfather");
    write(
        &bottle.join("bottle.yml"),
        "Name: Godfather Bottle\nArch: win64\n",
    );
    fake_game(&bottle.join("drive_c/Program Files/EA Games/The Godfather II"));

    // Not a game: an executable without game files.
    let decoy = home.join(".local/share/wineprefixes/decoy");
    write(&decoy.join("system.reg"), "WINE REGISTRY Version 2\n");
    write(
        &decoy.join("drive_c/Program Files/EA Games/The Godfather II/godfather2.exe"),
        b"MZ",
    );

    let roots = SearchRoots {
        home: Some(home),
        data_home: Some(data),
        config_home: Some(config),
        wine_prefix: None,
        extra: Vec::new(),
        windows: false,
    };
    Tree {
        _dir: dir,
        root,
        roots,
    }
}

fn find<'a>(installs: &'a [Install], suffix: &str) -> &'a Install {
    installs
        .iter()
        .find(|i| i.path.ends_with(suffix))
        .unwrap_or_else(|| panic!("{suffix} not found in {installs:#?}"))
}

#[cfg(unix)]
#[test]
fn finds_every_kind_of_install() {
    let t = tree();
    let installs = detect(&t.roots);
    assert_eq!(installs.len(), 7, "{installs:#?}");

    let steam = find(&installs, "library/steamapps/common/The Godfather II");
    assert_eq!(steam.profile, "The Godfather II (PC)");
    assert_eq!(steam.launch[0], LaunchMethod::Steam { app_id: 24830 });
    assert!(steam.found_by.iter().all(|o| o.kind == OriginKind::Steam));

    let proton = find(
        &installs,
        "pfx/drive_c/Program Files (x86)/EA Games/The Godfather II",
    );
    assert_eq!(proton.found_by[0].kind, OriginKind::Proton);
    assert_eq!(
        proton.launch[0],
        LaunchMethod::SteamShortcut {
            app_id: 2_934_567_890
        }
    );

    let shortcut = find(&installs, "elsewhere/GF2 copy");
    assert_eq!(
        shortcut.launch[0],
        LaunchMethod::SteamShortcut {
            app_id: 3_000_000_000
        }
    );

    let wine = find(&installs, "d-drive/Games/GF2");
    assert_eq!(wine.found_by[0].kind, OriginKind::Wine);
    assert!(wine.found_by[0].detail.contains("the godfather ii"));
    assert!(matches!(
        &wine.launch[0],
        LaunchMethod::Wine { prefix: Some(p), .. } if p.ends_with("home/.wine")
    ));

    let lutris = find(&installs, "Games/the-godfather-ii/drive_c/GF2");
    assert_eq!(lutris.sources(), "Lutris");
    assert_eq!(
        lutris.launch[0],
        LaunchMethod::Lutris {
            game: "the-godfather-ii".into()
        }
    );

    let heroic = find(&installs, "heroic/Godfather");
    assert_eq!(
        heroic.launch[0],
        LaunchMethod::Heroic {
            app: "gf2sideload".into(),
            runner: "sideload".into()
        }
    );

    let bottles = find(
        &installs,
        "bottles/Godfather/drive_c/Program Files/EA Games/The Godfather II",
    );
    assert_eq!(
        bottles.launch[0],
        LaunchMethod::Bottles {
            bottle: "Godfather Bottle".into(),
            flatpak: false
        }
    );

    assert!(
        !installs
            .iter()
            .any(|i| i.path.to_string_lossy().contains("decoy"))
    );
    // Every install can at least be started by Wine.
    assert!(
        installs
            .iter()
            .all(|i| i.launch.iter().any(|l| l.kind() == "wine"))
    );
    let _ = &t.root;
}

#[test]
fn nothing_is_found_in_an_empty_home() {
    let dir = tempfile::tempdir().unwrap();
    let roots = SearchRoots {
        home: Some(dir.path().to_path_buf()),
        data_home: Some(dir.path().join(".local/share")),
        config_home: Some(dir.path().join(".config")),
        ..SearchRoots::default()
    };
    assert!(detect(&roots).is_empty());
}

#[test]
fn extra_folders_and_prefixes_are_checked() {
    let dir = tempfile::tempdir().unwrap();
    let game = dir.path().join("copy");
    fake_game(&game);
    let prefix = dir.path().join("prefix");
    fake_game(&prefix.join("drive_c/Games/The Godfather II"));
    let roots = SearchRoots {
        extra: vec![game.clone(), prefix.clone(), dir.path().join("missing")],
        ..SearchRoots::default()
    };
    let installs = detect(&roots);
    assert_eq!(installs.len(), 2, "{installs:#?}");
    assert_eq!(find(&installs, "copy").found_by[0].kind, OriginKind::Folder);
}

#[test]
fn windows_paths_map_into_prefixes() {
    let dir = tempfile::tempdir().unwrap();
    let prefix = dir.path();
    fs::create_dir_all(prefix.join("drive_c")).unwrap();
    assert_eq!(
        windows_path_in_prefix(prefix, "C:\\Program Files\\EA Games\\GF2\\"),
        Some(prefix.join("drive_c/Program Files/EA Games/GF2"))
    );
    assert_eq!(windows_path_in_prefix(prefix, "C:\\..\\etc"), None);
    assert_eq!(windows_path_in_prefix(prefix, "Q:\\nothing"), None);
    assert_eq!(windows_path_in_prefix(prefix, "relative\\path"), None);
}

use std::fs;

use super::*;

/// A PE header like the game's: `Characteristics` 0x0103.
fn pe() -> Vec<u8> {
    let mut d = vec![0u8; 0x400];
    d[..2].copy_from_slice(b"MZ");
    d[0x3C..0x40].copy_from_slice(&0x100u32.to_le_bytes());
    d[0x100..0x104].copy_from_slice(b"PE\0\0");
    d[0x100 + 22..0x100 + 24].copy_from_slice(&0x0103u16.to_le_bytes());
    d
}

#[test]
fn large_address_aware_flag() {
    let exe = pe();
    assert_eq!(is_large_address_aware(&exe), Some(false));
    let flagged = with_large_address_aware(&exe).unwrap();
    assert_eq!(is_large_address_aware(&flagged), Some(true));
    assert_eq!(&flagged[0x116..0x118], &0x0123u16.to_le_bytes());
    // Only those two bytes differ.
    let diff = exe.iter().zip(&flagged).filter(|(a, b)| a != b).count();
    assert_eq!(diff, 1);
    assert!(with_large_address_aware(b"not a PE").is_err());
    assert_eq!(is_large_address_aware(b"MZ"), None);
}

#[test]
fn options_make_the_module_configuration() {
    let o = GraphicsOptions::default();
    assert!(o.is_default() && !o.wants_module());
    assert_eq!(o.config(), RenderConfig::default());
    let o = GraphicsOptions {
        module: true,
        fps: 5,
        fov: 75,
        fxaa: true,
        sharpen: 400,
        draw_merge: true,
        laa: true,
        ..Default::default()
    };
    let c = o.config();
    assert!(c.draw_merge == 1 && c.draws() && !RenderConfig::default().draws());
    // Clamped like the module does.
    assert_eq!((c.fps, c.fov, c.sharpen), (10, 75, 100));
    assert!(c.fxaa && c.time_of_day && c.memory_log == 30);
    let json = serde_json::to_string(&o).unwrap();
    assert_eq!(serde_json::from_str::<GraphicsOptions>(&json).unwrap(), o);
    // Older settings without the section.
    assert_eq!(
        serde_json::from_str::<GraphicsOptions>("{}").unwrap(),
        GraphicsOptions::default()
    );
}

#[test]
fn install_for_a_launch_and_take_away() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let c = RenderConfig {
        fps: 60,
        ..Default::default()
    };
    let install = RenderInstall::write(root, b"MZ render", &c).unwrap();
    assert_eq!(
        fs::read(root.join("scripts/doctor_render.asi")).unwrap(),
        b"MZ render"
    );
    assert_eq!(
        RenderConfig::parse(&fs::read_to_string(root.join("scripts/doctor_render.cfg")).unwrap()),
        c
    );
    assert!(!permanent(root));
    fs::write(root.join("scripts/doctor_render.log"), "log").unwrap();
    assert!(install.restore().unwrap());
    assert!(!root.join("scripts").exists());
    assert_eq!(last_log(root).unwrap(), "log");

    // A player's own module: kept by a launch, refused for another.
    fs::create_dir_all(root.join("scripts")).unwrap();
    fs::write(root.join("scripts/keep.asi"), "x").unwrap();
    install_permanent(root, b"MZ own", &c).unwrap();
    assert!(permanent(root));
    assert!(RenderInstall::write(root, b"MZ render", &c).is_err());
    assert!(!restore_stale(root).unwrap());
    assert!(root.join("scripts/doctor_render.asi").is_file());
    assert!(uninstall_permanent(root).unwrap());
    assert!(!root.join("scripts/doctor_render.asi").exists());
    assert!(root.join("scripts/keep.asi").is_file());

    // A crash of the toolkit: the next launch takes it away, and a changed
    // configuration stays.
    let install = RenderInstall::write(root, b"MZ render", &c).unwrap();
    std::mem::forget(install);
    fs::write(root.join("scripts/doctor_render.cfg"), "fps=30").unwrap();
    assert!(restore_stale(root).unwrap());
    assert!(!root.join("scripts/doctor_render.asi").exists());
    assert!(root.join("scripts/doctor_render.cfg").is_file());
}

#[test]
fn laa_only_where_the_executable_is_ours_to_start() {
    assert!(
        laa_problem(&LaunchMethod::Wine {
            prefix: None,
            wine: None,
            dll_overrides: None
        })
        .is_none()
    );
    assert!(laa_problem(&LaunchMethod::Direct).is_none());
    assert!(laa_problem(&LaunchMethod::Steam { app_id: 1 }).is_some());
    assert!(laa_problem(&LaunchMethod::Lutris { game: "gf2".into() }).is_some());
}

#[test]
fn plans_point_at_the_mirror() {
    let mut plan = LaunchPlan {
        program: "wine".into(),
        args: vec![
            "explorer".into(),
            "/desktop=x,1x1".into(),
            "/games/gf2/godfather2.exe".into(),
        ],
        env: Vec::new(),
        dir: "/games/gf2".into(),
        detached: false,
    };
    retarget(
        &mut plan,
        Path::new("/games/gf2"),
        Path::new("/cache/m/game"),
    );
    assert_eq!(plan.args[2], "/cache/m/game/godfather2.exe");
    assert_eq!(plan.args[0], "explorer");
    assert_eq!(plan.dir, Path::new("/cache/m/game"));
    let mut direct = LaunchPlan {
        program: "/games/gf2/godfather2.exe".into(),
        args: Vec::new(),
        env: Vec::new(),
        dir: "/games/gf2".into(),
        detached: false,
    };
    retarget(&mut direct, Path::new("/games/gf2"), Path::new("/m"));
    assert_eq!(direct.program, "/m/godfather2.exe");
}

#[cfg(unix)]
#[test]
fn mirror_with_a_flagged_copy_leaves_the_game_alone() {
    let (dir, game) = crate::workspace::tests::fake_game();
    let root = game.root().to_path_buf();
    fs::write(root.join("godfather2.exe"), pe()).unwrap();
    fs::create_dir_all(root.join("scripts")).unwrap();
    fs::write(root.join("scripts/doctor_live.asi"), "live").unwrap();
    let before = fs::read(root.join("godfather2.exe")).unwrap();
    let mirror_dir = dir.path().join("../mirror-test-laa");
    let mirror = LaaMirror::build(&game, &mirror_dir).unwrap();
    let exe = mirror.root().join("godfather2.exe");
    assert!(!fs::symlink_metadata(&exe).unwrap().file_type().is_symlink());
    assert_eq!(is_large_address_aware(&fs::read(&exe).unwrap()), Some(true));
    // Everything else is a link to the game.
    let live = mirror.root().join("scripts/doctor_live.asi");
    assert!(
        fs::symlink_metadata(&live)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(fs::read(root.join("godfather2.exe")).unwrap(), before);
    // The module's log, written next to the module in the mirror, comes
    // back to the game's scripts/ for its launch to take away.
    fs::write(mirror.root().join("scripts/doctor_live.log"), "log").unwrap();
    mirror.finish(&root).unwrap();
    assert!(!mirror_dir.exists());
    assert_eq!(
        fs::read_to_string(root.join("scripts/doctor_live.log")).unwrap(),
        "log"
    );
}

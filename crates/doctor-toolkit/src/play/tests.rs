//! Launch plans and sessions with fake games and launchers: nothing here
//! starts the real game or shows a window.

use std::fs;
use std::path::Path;

use super::*;

fn fake_game(dir: &Path) {
    fs::create_dir_all(dir.join("gf2")).unwrap();
    fs::write(dir.join("godfather2.exe"), b"MZ").unwrap();
    fs::write(dir.join("gf2/frontend.toc"), b"toc").unwrap();
}

#[cfg(unix)]
fn script(path: &Path, body: &str) {
    use std::os::unix::fs::PermissionsExt;
    fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

/// Polls without scanning the processes of the machine: only the launched
/// program counts.
fn run_to_end(session: &mut Session) -> Vec<SessionState> {
    let mut states = Vec::new();
    for _ in 0..200 {
        let state = session.poll(&[]);
        if states.last() != Some(&state) {
            states.push(state.clone());
        }
        if !matches!(state, SessionState::Starting | SessionState::Running(_)) {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    states
}

#[test]
fn words_split_like_a_shell() {
    assert_eq!(
        split_words(r#"wine "{exe}" -windowed 'a b' c\ d"#).unwrap(),
        ["wine", "{exe}", "-windowed", "a b", "c d"]
    );
    assert_eq!(split_words(r#""" x"#).unwrap(), ["", "x"]);
    assert!(split_words("'unclosed").is_err());
    assert!(split_words("   ").unwrap().is_empty());
}

#[test]
fn plans_name_the_launchers() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    fake_game(root);
    let shortcut = plan(
        &LaunchMethod::SteamShortcut {
            app_id: 3_000_000_000,
        },
        root,
    )
    .unwrap();
    assert!(shortcut.detached);
    let url = (3_000_000_000u64 << 32) | 0x0200_0000;
    assert!(
        shortcut
            .args
            .iter()
            .any(|a| a.to_string_lossy() == format!("steam://rungameid/{url}"))
    );
    let heroic = plan(
        &LaunchMethod::Heroic {
            app: "gf 2".into(),
            runner: "sideload".into(),
        },
        root,
    )
    .unwrap();
    assert!(
        heroic
            .args
            .iter()
            .any(|a| a.to_string_lossy() == "heroic://launch?appName=gf%202&runner=sideload")
    );
    let bottles = plan(
        &LaunchMethod::Bottles {
            bottle: "B".into(),
            flatpak: true,
        },
        root,
    )
    .unwrap();
    assert_eq!(bottles.program, "flatpak");
    assert_eq!(bottles.args[2], "com.usebottles.bottles");
    let lutris = plan(&LaunchMethod::Lutris { game: "gf2".into() }, root).unwrap();
    assert!(lutris.args.iter().any(|a| a == "lutris:rungame/gf2"));
    let proton = plan(
        &LaunchMethod::Proton {
            proton: root.join("Proton 9.0"),
            compat_data: root.join("compat"),
        },
        root,
    )
    .unwrap();
    assert_eq!(proton.args[0], "run");
    assert!(
        proton
            .env
            .iter()
            .any(|(k, _)| k == "STEAM_COMPAT_DATA_PATH")
    );
    assert!(!proton.detached);
    let direct = plan(&LaunchMethod::Direct, root).unwrap();
    assert_eq!(Path::new(&direct.program), root.join("godfather2.exe"));
    assert!(plan(&LaunchMethod::Auto, root).is_err());
    assert!(plan(&LaunchMethod::Direct, &root.join("gf2")).is_err());
    assert!(
        plan(&LaunchMethod::Command { command: "".into() }, root).is_err(),
        "an empty command"
    );
}

#[cfg(unix)]
#[test]
fn wine_gets_the_prefix_and_the_asi_loader_override() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("game");
    fake_game(&root);
    fs::write(root.join("dinput8.dll"), b"MZ").unwrap();
    let record = dir.path().join("record.txt");
    let wine = dir.path().join("fake-wine");
    script(
        &wine,
        &format!(
            "echo \"$WINEPREFIX|$WINEDLLOVERRIDES|$(pwd)|$*\" > '{}'",
            record.display()
        ),
    );
    let method = LaunchMethod::Wine {
        prefix: Some(dir.path().join("prefix")),
        wine: Some(wine.to_string_lossy().into_owned()),
        dll_overrides: None,
    };
    let plan = plan(&method, &root).unwrap();
    assert!(plan.command_line().contains("WINEDLLOVERRIDES=dinput8=n,b"));
    let mut session = Session::start(&plan).unwrap();
    let states = run_to_end(&mut session);
    assert_eq!(states.last(), Some(&SessionState::Exited), "{states:?}");
    let line = fs::read_to_string(&record).unwrap();
    let fields: Vec<&str> = line.trim().split('|').collect();
    assert_eq!(fields[0], dir.path().join("prefix").to_string_lossy());
    assert_eq!(fields[1], "dinput8=n,b");
    assert_eq!(
        fs::canonicalize(fields[2]).unwrap(),
        fs::canonicalize(&root).unwrap()
    );
    assert!(fields[3].ends_with("godfather2.exe"));
}

#[cfg(unix)]
#[test]
fn a_fake_game_runs_and_exits() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("game");
    fake_game(&root);
    script(&root.join("fake-game.sh"), "sleep 0.5");
    let method = LaunchMethod::Command {
        command: "{dir}/fake-game.sh --exe {exe}".into(),
    };
    let plan = plan(&method, &root).unwrap();
    assert_eq!(plan.args.len(), 2);
    let mut session = Session::start(&plan).unwrap();
    let pid = session.pid().unwrap();
    let states = run_to_end(&mut session);
    assert_eq!(
        states,
        [SessionState::Running(vec![pid]), SessionState::Exited],
        "{states:?}"
    );
}

#[cfg(unix)]
#[test]
fn a_failing_launcher_is_reported() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("game");
    fake_game(&root);
    script(&root.join("broken.sh"), "exit 3");
    let plan = plan(
        &LaunchMethod::Command {
            command: "{dir}/broken.sh".into(),
        },
        &root,
    )
    .unwrap();
    let mut session = Session::start(&plan).unwrap();
    let states = run_to_end(&mut session);
    assert!(
        matches!(states.last(), Some(SessionState::NotStarted(r)) if r.contains("exited")),
        "{states:?}"
    );
    // A detached launcher whose game never shows up times out.
    let mut detached = plan.clone();
    detached.detached = true;
    detached.program = "true".into();
    detached.args.clear();
    let mut session = Session::start(&detached).unwrap();
    session.start_timeout = Duration::ZERO;
    std::thread::sleep(Duration::from_millis(100));
    assert!(matches!(
        session.poll(&[]),
        SessionState::NotStarted(r) if r.contains("did not start")
    ));
    // Seen in the process list, then gone: exited.
    let mut session = Session::start(&detached).unwrap();
    assert_eq!(session.poll(&[42]), SessionState::Running(vec![42]));
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(session.poll(&[]), SessionState::Exited);
}

#[test]
fn processes_are_recognised_by_their_program() {
    let names = ["godfather2.exe"];
    let cmd = |parts: &[&str]| parts.join("\0").into_bytes();
    assert!(is_named(&cmd(&["/games/godfather2.exe"]), "", &names));
    assert!(is_named(
        &cmd(&["C:\\Program Files\\EA Games\\The Godfather II\\GodFather2.exe"]),
        "godfather2.exe",
        &names
    ));
    assert!(!is_named(&cmd(&["wine", "godfather2.exe"]), "wine", &names));
    assert!(!is_named(
        &cmd(&["doctor-toolkit", "play", "/g/godfather2.exe"]),
        "doctor-toolkit",
        &names
    ));
    assert!(is_named(b"", "godfather2.exe", &names));
    assert!(!is_named(b"", "", &names));

    let dir = tempfile::tempdir().unwrap();
    for (pid, cmdline, comm) in [
        (
            "101",
            cmd(&["Z:\\g\\godfather2.exe", "-x"]),
            "godfather2.exe",
        ),
        ("102", cmd(&["/usr/bin/wine", "godfather2.exe"]), "wine"),
        ("self", cmd(&["godfather2.exe"]), "godfather2.exe"),
    ] {
        fs::create_dir_all(dir.path().join(pid)).unwrap();
        fs::write(dir.path().join(pid).join("cmdline"), cmdline).unwrap();
        fs::write(dir.path().join(pid).join("comm"), format!("{comm}\n")).unwrap();
    }
    assert_eq!(scan_proc(dir.path(), &names), [101]);
    assert_eq!(
        parse_tasklist(
            "\"godfather2.exe\",\"4242\",\"Console\",\"1\",\"900,000 K\"\r\n\"x.exe\",\"1\"\r\n",
            &names
        ),
        [4242]
    );
}

#[cfg(target_os = "linux")]
#[test]
fn a_dummy_game_process_is_seen() {
    use std::os::unix::process::CommandExt;
    let dir = tempfile::tempdir().unwrap();
    let sleep = ["/bin/sleep", "/usr/bin/sleep"]
        .into_iter()
        .find(|p| Path::new(p).is_file())
        .expect("sleep");
    // A native process named godfather2.exe, and one whose program is a
    // Windows path, as under Wine.
    let native = dir.path().join("godfather2.exe");
    fs::copy(sleep, &native).unwrap();
    let mut first = std::process::Command::new(&native)
        .arg("30")
        .spawn()
        .unwrap();
    let mut second = std::process::Command::new(sleep)
        .arg0("C:\\Program Files (x86)\\EA Games\\The Godfather II\\godfather2.exe")
        .arg("30")
        .spawn()
        .unwrap();
    std::thread::sleep(Duration::from_millis(200));
    let found = game_processes();
    let _ = first.kill();
    let _ = second.kill();
    let _ = first.wait();
    let _ = second.wait();
    assert!(found.contains(&first.id()), "{found:?}");
    assert!(found.contains(&second.id()), "{found:?}");
    assert!(!game_processes().contains(&first.id()));
}

#[test]
fn the_watch_reports_changes() {
    let state = Arc::new(Mutex::new(Vec::<u32>::new()));
    let changes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let (scan_state, counter) = (Arc::clone(&state), Arc::clone(&changes));
    let watch = GameWatch::start_with(
        Duration::from_millis(10),
        move || scan_state.lock().unwrap().clone(),
        move || {
            counter.fetch_add(1, Ordering::SeqCst);
        },
    );
    assert!(watch.running().is_empty());
    *state.lock().unwrap() = vec![7];
    let deadline = Instant::now() + Duration::from_secs(5);
    while watch.running().is_empty() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(watch.running(), [7]);
    assert!(changes.load(Ordering::SeqCst) >= 1);
    let disabled = GameWatch::disabled();
    disabled.set_running(vec![1]);
    assert_eq!(disabled.running(), [1]);
}

#[test]
fn methods_are_stored_as_tables() {
    let config = PlayConfig {
        method: LaunchMethod::Wine {
            prefix: Some("/p".into()),
            wine: None,
            dll_overrides: None,
        },
        apply_before: true,
        revert_after: false,
        start: Default::default(),
        live: Default::default(),
        dxvk: false,
    };
    let json = serde_json::to_string(&config).unwrap();
    assert_eq!(
        json,
        r#"{"method":{"method":"wine","prefix":"/p"},"apply_before":true,"revert_after":false}"#
    );
    assert_eq!(serde_json::from_str::<PlayConfig>(&json).unwrap(), config);
    // Starting in a level is kept with the rest (and is absent when unused).
    let level = PlayConfig {
        start: start::StartOptions {
            streams: vec!["gf2/cuba".into()],
            spawn: Some("26d7c8323dfa11aa464f574c45523130".into()),
            ..Default::default()
        },
        ..config
    };
    let json = serde_json::to_string(&level).unwrap();
    assert!(
        json.ends_with(
            r#""start":{"streams":["gf2/cuba"],"spawn":"26d7c8323dfa11aa464f574c45523130"}}"#
        ),
        "{json}"
    );
    assert_eq!(serde_json::from_str::<PlayConfig>(&json).unwrap(), level);
    // The editor keeps its settings as TOML.
    let window = PlayConfig {
        start: start::StartOptions {
            windowed: Some(true),
            resolution: Some((1280, 720)),
            ..level.start.clone()
        },
        ..level
    };
    let text = toml::to_string_pretty(&window).unwrap();
    assert!(
        text.contains("resolution = [\n    1280,\n    720,\n]"),
        "{text}"
    );
    assert_eq!(toml::from_str::<PlayConfig>(&text).unwrap(), window);
    assert_eq!(
        serde_json::from_str::<PlayConfig>("{}").unwrap(),
        PlayConfig::default()
    );
}

#[test]
fn the_fallback_finds_the_prefix_around_the_game() {
    let dir = tempfile::tempdir().unwrap();
    let prefix = dir.path().join("pfx");
    let game = prefix.join("drive_c/Games/GF2");
    fake_game(&game);
    fs::write(prefix.join("system.reg"), "WINE REGISTRY Version 2\n").unwrap();
    if cfg!(windows) {
        assert_eq!(fallback_method(&game), LaunchMethod::Direct);
    } else {
        assert_eq!(
            fallback_method(&game),
            LaunchMethod::Wine {
                prefix: Some(prefix),
                wine: None,
                dll_overrides: None
            }
        );
        assert_eq!(auto_method(&game, &[]), fallback_method(&game));
    }
}

use std::time::Duration;

use super::fake::FakeGame;
use super::*;

const T: Duration = Duration::from_secs(5);

fn config(token: &str, writes: bool) -> Config {
    Config {
        port: 0,
        token: token.into(),
        writes,
    }
}

#[test]
fn reads_through_the_client() {
    let game = FakeGame::start(config("secret", true)).unwrap();
    let mut c = Client::connect(&game.endpoint("secret"), "test", T).unwrap();
    assert!(c.hello().edition_ok);
    assert!(c.hello().writes);
    let s = c.status().unwrap();
    assert_eq!(s.state, proto::LoopState::Game);
    assert!(s.player);
    let p = c.player().unwrap().unwrap();
    assert_eq!(p.position, [283.9, -0.07, 371.8]);
    assert!(c.camera().unwrap().unwrap().fov.is_some());
    assert_eq!(c.streams().unwrap().len(), 2);
    let near = c.objects(None, 5.0, None, 10).unwrap();
    assert_eq!(near.len(), 2);
    assert!(near[0].distance <= near[1].distance);
    let switches = c
        .objects(None, 100.0, Some(proto::name_hash("VariableSwitch")), 10)
        .unwrap();
    assert_eq!(switches.len(), 1);
    assert_eq!(c.peek(0x0122_33A0, 4).unwrap().len(), 4);
    assert_eq!(c.peek(0x10, 4).unwrap_err().code(), Some(ErrorCode::Failed));
}

#[test]
fn writes_through_the_client() {
    let game = FakeGame::start(config("", true)).unwrap();
    let mut c = Client::connect(&game.endpoint(""), "test", T).unwrap();
    let done = c.teleport([1.0, 2.0, 3.0], Some(0.0)).unwrap();
    assert_eq!(done.count, 1);
    assert_eq!(c.player().unwrap().unwrap().position, [1.0, 2.0, 3.0]);
    assert_eq!(
        c.send(MessageId::Name("doctor_ping".into()), None)
            .unwrap()
            .count,
        2
    );
    assert_eq!(
        c.send(MessageId::Name("doctor_ping".into()), Some([1; 16]))
            .unwrap()
            .count,
        1
    );
    assert_eq!(game.world.lock().unwrap().sent.len(), 2);
    let l = c.lua("return GetSimTime()").unwrap();
    assert!(l.ok);
    let bad = c.lua("this is not lua").unwrap();
    assert!(!bad.ok);
}

#[test]
fn refusals_are_errors() {
    // Wrong token.
    let game = FakeGame::start(config("secret", false)).unwrap();
    let err = Client::connect(&game.endpoint("other"), "test", T)
        .err()
        .unwrap();
    assert_eq!(err.code(), Some(ErrorCode::Denied));
    // Writes switched off.
    let mut c = Client::connect(&game.endpoint("secret"), "test", T).unwrap();
    assert!(!c.hello().writes);
    let err = c.teleport([0.0; 3], None).unwrap_err();
    assert_eq!(err.code(), Some(ErrorCode::Denied));
    // Another build: Hello says so, everything else is refused.
    game.world.lock().unwrap().wrong_edition = true;
    let mut c = Client::connect(&game.endpoint("secret"), "test", T).unwrap();
    assert!(!c.hello().edition_ok);
    assert_eq!(c.status().unwrap_err().code(), Some(ErrorCode::Edition));
}

#[test]
fn nobody_listening_is_unreachable() {
    let port = {
        let l = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        l.local_addr().unwrap().port()
    };
    let err = Client::connect(
        &Endpoint {
            port,
            token: String::new(),
        },
        "test",
        Duration::from_millis(500),
    )
    .err()
    .unwrap();
    assert!(matches!(err, LinkError::Unreachable(_)));
}

#[test]
fn another_protocol_version_is_refused() {
    // A server that answers every frame with version 99.
    let listener = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        let (mut s, _) = listener.accept().unwrap();
        let (h, _) = read_frame(&mut s).unwrap();
        let mut frame = Response::Status(Status {
            frame: 0,
            state: proto::LoopState::Unknown,
            player: false,
            sim_time_ms: None,
            time_of_day: None,
        })
        .to_frame(proto::kind::STATUS, h.id);
        frame[4] = 99;
        s.write_all(&frame).unwrap();
    });
    let err = Client::connect(
        &Endpoint {
            port,
            token: String::new(),
        },
        "test",
        T,
    )
    .err()
    .unwrap();
    assert_eq!(err.code(), Some(ErrorCode::Version));
}

#[test]
fn install_and_restore() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let cfg = config("tok", true);
    let install = ModuleInstall::write(root, b"MZ module", &cfg).unwrap();
    assert!(installed(root));
    assert_eq!(
        std::fs::read(root.join("scripts/doctor_live.asi")).unwrap(),
        b"MZ module"
    );
    assert_eq!(Endpoint::of_game(root).token, "tok");
    // The module wrote its log.
    std::fs::write(root.join("scripts/doctor_live.log"), "loaded").unwrap();
    assert!(install.restore().unwrap());
    assert!(
        !root.join("scripts").exists(),
        "scripts/ was made by the toolkit"
    );
    assert_eq!(
        std::fs::read_to_string(root.join(LAST_LOG)).unwrap(),
        "loaded"
    );
    assert!(!installed(root));
    assert!(!restore_stale(root).unwrap());
}

#[test]
fn restore_keeps_the_players_files() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir(root.join("scripts")).unwrap();
    std::fs::write(root.join("scripts/GF2ASI.asi"), "theirs").unwrap();
    {
        let _install = ModuleInstall::write(root, b"MZ", &config("", false)).unwrap();
        // Someone replaced the configuration meanwhile: it stays.
        std::fs::write(root.join("scripts/doctor_live.cfg"), "port=1").unwrap();
        // Dropped without restore: Drop takes the module away.
    }
    assert!(!root.join("scripts/doctor_live.asi").exists());
    assert_eq!(
        std::fs::read_to_string(root.join("scripts/doctor_live.cfg")).unwrap(),
        "port=1"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("scripts/GF2ASI.asi")).unwrap(),
        "theirs"
    );
    // Not ours: refused.
    std::fs::write(root.join("scripts/doctor_live.asi"), "other").unwrap();
    assert!(ModuleInstall::write(root, b"MZ", &config("", false)).is_err());
}

#[test]
fn stale_install_is_taken_away() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let install = ModuleInstall::write(root, b"MZ", &config("", false)).unwrap();
    std::mem::forget(install); // the toolkit died while the game ran
    assert!(installed(root));
    assert!(restore_stale(root).unwrap());
    assert!(!root.join("scripts").exists());
}

#[test]
fn finds_the_loader() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    assert_eq!(loader::find(root), None);
    std::fs::write(root.join("winmm.dll"), "a system DLL").unwrap();
    assert_eq!(loader::find(root), None);
    std::fs::write(root.join("DINPUT8.dll"), b"xx Ultimate ASI Loader xx").unwrap();
    std::fs::create_dir(root.join("scripts")).unwrap();
    std::fs::write(root.join("scripts/GF2ASI.asi"), "").unwrap();
    let l = loader::find(root).unwrap();
    assert!(l.ultimate);
    assert_eq!(l.modules, ["scripts/GF2ASI.asi"]);
    assert_eq!(l.wine_override(), "dinput8=n,b");
}

#[test]
fn tokens_differ() {
    assert_ne!(new_token(), new_token());
    assert_eq!(new_token().len(), 32);
}

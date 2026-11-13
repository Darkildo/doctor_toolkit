use std::time::Duration;

use super::fake::FakeGame;
use super::*;

const T: Duration = Duration::from_secs(5);

fn config(token: &str, writes: bool) -> Config {
    Config {
        port: 0,
        token: token.into(),
        writes,
        fix_input: true,
        journal: true,
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
        3
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

#[test]
fn a_start_in_a_level_installs_the_module_for_the_input() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    // Without a loader nothing is written: the module could not load.
    assert!(matches!(
        install_for_input(root).unwrap(),
        InputFix::NoLoader
    ));
    assert!(!root.join("scripts").exists());
    std::fs::write(root.join("dinput8.dll"), b"xx Ultimate ASI Loader xx").unwrap();
    match install_for_input(root).unwrap() {
        InputFix::Installed(install) => {
            // Only the input fix: no teleport, messages or Lua.
            assert!(!install.config().writes);
            assert!(install.config().fix_input);
            let text = std::fs::read_to_string(root.join("scripts/doctor_live.cfg")).unwrap();
            assert!(text.contains("fix_input=1") && text.contains("writes=0"));
            assert!(install.restore().unwrap());
            assert!(!root.join("scripts").exists());
        }
        // The module is not built on this machine.
        InputFix::NoModule => assert!(module_binary().is_none()),
        InputFix::NoLoader => panic!("the loader is there"),
    }
}

#[test]
fn the_journal_through_the_client() {
    use proto::journal::{EventBody, OutputSource, mask};
    let game = FakeGame::start(config("", true)).unwrap();
    let mut c = Client::connect(&game.endpoint(""), "test", T).unwrap();
    assert_eq!(c.version(), proto::PROTOCOL_VERSION);
    assert!(c.has_journal());
    let first = c.events(0, mask::ALL, 100).unwrap();
    assert!(first.events.is_empty());
    assert_eq!(first.recording, mask::ALL_V3);
    c.lua("print(\"hello\")").unwrap();
    assert!(!c.lua("this is bad").unwrap().ok);
    c.send(MessageId::Name("doctor_ping".into()), None).unwrap();
    let b = c.events(first.next - 1, mask::ALL, 100).unwrap();
    assert_eq!(b.events.len(), 3, "{b:?}");
    assert!(matches!(
        &b.events[0].body,
        EventBody::Output { source: OutputSource::Print, text, .. } if text == "hello"
    ));
    assert!(matches!(
        &b.events[1].body,
        EventBody::LuaError { status: 3, .. }
    ));
    let EventBody::Message {
        receivers,
        listeners,
        ..
    } = &b.events[2].body
    else {
        panic!("not a message: {:?}", b.events[2]);
    };
    // Two with a position and the logic object.
    assert_eq!(*listeners, 3);
    assert!(receivers.contains(&[4; 16]));
    // Only errors; nothing new after the last one.
    let errors = c.events(0, mask::LUA_ERROR, 100).unwrap();
    assert_eq!(errors.events.len(), 1);
    assert!(
        c.events(b.next - 1, mask::ALL, 100)
            .unwrap()
            .events
            .is_empty()
    );
}

#[test]
fn every_object_through_the_client() {
    let game = FakeGame::start(config("", false)).unwrap();
    let mut c = Client::connect(&game.endpoint(""), "test", T).unwrap();
    let all = c.world(proto::WorldFilter::default(), 100).unwrap();
    assert_eq!(all.total, 5);
    assert_eq!(all.objects.len(), 5);
    assert_eq!(all.stream_name(1), Some("gf2\\cuba.str"));
    let logic = c
        .world(
            proto::WorldFilter {
                positioned: Some(false),
                ..Default::default()
            },
            100,
        )
        .unwrap();
    assert_eq!(logic.objects.len(), 2);
    assert!(logic.objects.iter().all(|o| o.position.is_none()));
    assert_eq!(
        logic.class_name(logic.objects[0].class),
        Some("VariableSwitch")
    );
    let one = c
        .world(
            proto::WorldFilter {
                guids: vec![[5; 16]],
                ..Default::default()
            },
            100,
        )
        .unwrap();
    assert_eq!(one.objects.len(), 1);
    assert!(!one.objects[0].listening);
    // `max` cuts the list, `total` still counts.
    let cut = c.world(proto::WorldFilter::default(), 2).unwrap();
    assert_eq!((cut.total, cut.objects.len()), (5, 2));
}

#[test]
fn a_module_of_protocol_1_still_answers() {
    let game = FakeGame::start(config("t", false)).unwrap();
    game.world.lock().unwrap().protocol = 1;
    let mut c = Client::connect(&game.endpoint("t"), "test", T).unwrap();
    assert_eq!(c.version(), 1);
    assert!(!c.has_journal());
    assert!(c.status().unwrap().player);
    let err = c.events(0, proto::journal::mask::ALL, 10).unwrap_err();
    assert_eq!(err.code(), Some(ErrorCode::Unsupported));
}

#[test]
fn a_frame_of_version_1_cannot_carry_a_request_of_version_2() {
    let game = FakeGame::start(config("t", false)).unwrap();
    let mut s = std::net::TcpStream::connect(("127.0.0.1", game.port)).unwrap();
    s.write_all(
        &Request::Hello {
            token: "t".into(),
            client: "old".into(),
        }
        .to_frame_of(1, 1),
    )
    .unwrap();
    let (h, _) = read_frame(&mut s).unwrap();
    // A client of stage 34 gets its answers in version 1.
    assert_eq!(h.version, 1);
    s.write_all(
        &Request::Events {
            after: 0,
            kinds: 1,
            max: 1,
        }
        .to_frame_of(1, 2),
    )
    .unwrap();
    let (h, payload) = read_frame(&mut s).unwrap();
    assert_eq!(h.version, 1);
    let Response::Error(f) = Response::decode(h.kind, &payload).unwrap() else {
        panic!("not an error");
    };
    assert_eq!(f.code, ErrorCode::Unsupported);
}

#[test]
fn breakpoints_pause_and_step_through_the_client() {
    use proto::{BreakOn, BreakRule, Control, StopPlace, StopReason};
    let game = FakeGame::start(config("", true)).unwrap();
    let mut c = Client::connect(&game.endpoint(""), "test", T).unwrap();
    assert!(c.has_debug());
    let d = c.debug_state().unwrap();
    assert!(d.stop.is_none() && d.rules.is_empty());
    // A break on a message to one object.
    let ping = proto::name_hash("doctor_ping");
    c.set_breaks(vec![BreakRule::message(ping, Some([4; 16]))], 30)
        .unwrap();
    assert_eq!(c.debug_state().unwrap().hold_limit_s, 30);
    // The game's own traffic hits it in the bus: the game stands there.
    game.world.lock().unwrap().activity(0);
    let d = c.debug_state().unwrap();
    let stop = d.stop.unwrap();
    assert_eq!(stop.place, StopPlace::Bus);
    let StopReason::Message {
        hash, receivers, ..
    } = &stop.reason
    else {
        panic!("{stop:?}");
    };
    assert_eq!(*hash, ping);
    assert!(receivers.contains(&[4; 16]));
    assert_eq!(d.hits, 1);
    // Standing: the frame does not move, writes inside the game's code are
    // refused, reads answer.
    let frame = c.status().unwrap().frame;
    assert_eq!(c.status().unwrap().frame, frame);
    assert_eq!(
        c.teleport([0.0; 3], None).unwrap_err().code(),
        Some(ErrorCode::NotReady)
    );
    assert!(c.player().unwrap().is_some());
    c.control(Control::Continue).unwrap();
    assert!(c.debug_state().unwrap().stop.is_none());
    assert!(c.status().unwrap().frame > frame);
    // Pause and step stand in a frame, where writes run.
    c.control(Control::Pause).unwrap();
    let d = c.debug_state().unwrap();
    assert_eq!(d.stop.as_ref().unwrap().place, StopPlace::Frame);
    assert_eq!(c.teleport([1.0, 0.0, 1.0], None).unwrap().count, 1);
    let before = c.status().unwrap().frame;
    c.control(Control::Step).unwrap();
    let d = c.debug_state().unwrap();
    assert_eq!(d.stop.unwrap().reason, StopReason::Step);
    assert_eq!(c.status().unwrap().frame, before + 1);
    c.control(Control::Continue).unwrap();
    // A message the link sends stops in the next frame; a one-shot rule
    // goes away after its hit.
    c.set_breaks(
        vec![BreakRule {
            on: BreakOn::Message {
                hash: ping,
                receiver: None,
            },
            once: true,
        }],
        0,
    )
    .unwrap();
    c.send(MessageId::Name("doctor_ping".into()), None).unwrap();
    let d = c.debug_state().unwrap();
    assert_eq!(d.stop.unwrap().place, StopPlace::Frame);
    assert!(d.rules.is_empty());
    c.control(Control::Continue).unwrap();
    // A traced node of a visual script, and Lua errors.
    c.set_breaks(
        vec![
            BreakRule {
                on: BreakOn::Output {
                    text: "dt:doctor_demo:3;".into(),
                },
                once: false,
            },
            BreakRule {
                on: BreakOn::LuaError,
                once: false,
            },
        ],
        0,
    )
    .unwrap();
    game.world.lock().unwrap().activity(0);
    assert!(c.debug_state().unwrap().stop.is_none(), "node 2 printed");
    game.world.lock().unwrap().activity(1);
    let stop = c.debug_state().unwrap().stop.unwrap();
    assert_eq!(stop.place, StopPlace::Print);
    c.control(Control::Continue).unwrap();
    assert!(!c.lua("this is bad").unwrap().ok);
    assert!(matches!(
        c.debug_state().unwrap().stop.unwrap().reason,
        StopReason::LuaError { .. }
    ));
    c.control(Control::Continue).unwrap();
    c.set_breaks(Vec::new(), 0).unwrap();
    assert!(c.debug_state().unwrap().rules.is_empty());
}

#[test]
fn free_camera_and_texture_through_the_client() {
    let game = FakeGame::start(config("", true)).unwrap();
    let mut c = Client::connect(&game.endpoint(""), "test", T).unwrap();
    let mut view = c.camera().unwrap().unwrap();
    view.pose.position = [10.0, 20.0, 30.0];
    c.set_camera(Some(view)).unwrap();
    assert_eq!(
        c.camera().unwrap().unwrap().pose.position,
        [10.0, 20.0, 30.0]
    );
    assert_eq!(c.debug_state().unwrap().camera, Some(view));
    c.set_camera(None).unwrap();
    assert_ne!(
        c.camera().unwrap().unwrap().pose.position,
        [10.0, 20.0, 30.0]
    );
    let name = proto::name_hash(super::fake::FAKE_TEXTURE);
    assert_eq!(c.texture(name, vec![0; 84]).unwrap().count, 1);
    assert_eq!(
        c.texture(name, vec![0; 10]).unwrap_err().code(),
        Some(ErrorCode::Failed)
    );
    assert_eq!(
        c.texture(7, vec![0; 84]).unwrap_err().code(),
        Some(ErrorCode::Failed)
    );
    assert_eq!(game.world.lock().unwrap().replaced, [(name, 84)]);
    // Writes need writes=1; breakpoints are not writes.
    let ro = FakeGame::start(config("", false)).unwrap();
    let mut r = Client::connect(&ro.endpoint(""), "test", T).unwrap();
    assert_eq!(
        r.set_camera(None).unwrap_err().code(),
        Some(ErrorCode::Denied)
    );
    assert!(r.set_breaks(Vec::new(), 0).is_ok());
}

#[test]
fn counts_of_messages_go_to_new_clients_only() {
    use proto::journal::{EventBody, mask};
    let game = FakeGame::start(config("", true)).unwrap();
    game.world.lock().unwrap().activity(0);
    let mut c = Client::connect(&game.endpoint(""), "test", T).unwrap();
    let b = c.events(0, mask::ALL_V3, 100).unwrap();
    assert!(
        b.events
            .iter()
            .any(|e| matches!(e.body, EventBody::MessageCounts { .. }))
    );
    // A client of protocol 2 asks with every bit and still gets only what
    // it can read.
    let mut s = std::net::TcpStream::connect(("127.0.0.1", game.port)).unwrap();
    s.write_all(
        &Request::Hello {
            token: String::new(),
            client: "old".into(),
        }
        .to_frame_of(2, 1),
    )
    .unwrap();
    read_frame(&mut s).unwrap();
    s.write_all(
        &Request::Events {
            after: 0,
            kinds: 0xFF,
            max: 100,
        }
        .to_frame_of(2, 2),
    )
    .unwrap();
    let (h, payload) = read_frame(&mut s).unwrap();
    assert_eq!(h.version, 2);
    let Response::Events(b) = Response::decode(h.kind, &payload).unwrap() else {
        panic!("not events");
    };
    assert!(
        !b.events
            .iter()
            .any(|e| matches!(e.body, EventBody::MessageCounts { .. }))
    );
    assert_eq!(b.recording, mask::ALL);
}

#[test]
fn a_module_of_protocol_2_still_answers() {
    let game = FakeGame::start(config("t", true)).unwrap();
    game.world.lock().unwrap().protocol = 2;
    let mut c = Client::connect(&game.endpoint("t"), "test", T).unwrap();
    assert_eq!(c.version(), 2);
    assert!(c.has_journal() && !c.has_debug());
    assert!(c.events(0, proto::journal::mask::ALL, 10).is_ok());
    assert_eq!(
        c.debug_state().unwrap_err().code(),
        Some(ErrorCode::Unsupported)
    );
}

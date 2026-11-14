use super::*;

fn requests() -> Vec<Request> {
    vec![
        Request::Hello {
            token: "abc".into(),
            client: "doctor-toolkit 0.1.0".into(),
        },
        Request::Status,
        Request::Player,
        Request::Camera,
        Request::Streams,
        Request::Objects {
            center: Some([1.0, 2.0, -3.5]),
            radius: 30.0,
            class: Some(0x5555_E170),
            max: 50,
        },
        Request::Objects {
            center: None,
            radius: 10.0,
            class: None,
            max: 1,
        },
        Request::Teleport {
            position: [179.4, 21.2, -25.4],
            yaw: Some(1.5),
        },
        Request::Teleport {
            position: [0.0; 3],
            yaw: None,
        },
        Request::Send {
            message: MessageId::Name("doctor_ping".into()),
            target: Some([7; 16]),
        },
        Request::Send {
            message: MessageId::Hash(0xDEAD_BEEF),
            target: None,
        },
        Request::Lua {
            code: "return GetSimTime()".into(),
        },
        Request::Peek {
            address: 0x0122_33A0,
            len: 4,
        },
        Request::Events {
            after: 41,
            kinds: crate::journal::mask::ALL,
            max: 500,
        },
        Request::World {
            filter: WorldFilter {
                class: Some(0x5555_E170),
                guids: vec![[1; 16], [2; 16]],
                group: Some(7),
                positioned: Some(false),
            },
            max: 100,
        },
        Request::World {
            filter: WorldFilter::default(),
            max: 0,
        },
        Request::Break {
            rules: vec![
                BreakRule::message(0xABCD_0001, Some([3; 16])),
                BreakRule::message(0xABCD_0002, None),
                BreakRule {
                    on: BreakOn::Output {
                        text: "dt:hello:4;".into(),
                    },
                    once: true,
                },
                BreakRule {
                    on: BreakOn::LuaError,
                    once: false,
                },
            ],
            hold_s: 60,
        },
        Request::Break {
            rules: Vec::new(),
            hold_s: 0,
        },
        Request::Control(Control::Pause),
        Request::Control(Control::Step),
        Request::Debug,
        Request::SetCamera {
            camera: Some(CameraView {
                pose: Pose {
                    position: [1.0, 2.0, 3.0],
                    right: [1.0, 0.0, 0.0],
                    up: [0.0, 1.0, 0.0],
                    forward: [0.0, 0.0, 1.0],
                },
                fov: Some(50.0),
            }),
        },
        Request::SetCamera { camera: None },
        Request::Texture {
            name: 0x1234_5678,
            data: vec![1, 2, 3, 4, 5],
        },
        Request::TimeOfDay,
        Request::SetTime(TimeChange {
            time: Some([22, 30, 0]),
            rate: Some(0.0),
        }),
        Request::SetTime(TimeChange::default()),
        Request::Weather,
        Request::StreamLoad {
            name: "gf2\\cuba\\local".into(),
        },
        Request::StreamUnload {
            name: "gf2\\cuba\\local".into(),
            force: true,
        },
        Request::StreamQueue,
        Request::Money,
        Request::SetMoney(MoneyChange::Add(250.0)),
        Request::SetMoney(MoneyChange::Set(10_000.0)),
        Request::Family,
        Request::Mission,
        Request::Variables {
            hashes: vec![0x9381_3AB9, 0x001A_01D2],
        },
        Request::Variables { hashes: Vec::new() },
        Request::SetVariable {
            hash: 0x9381_3AB9,
            value: -3,
        },
        Request::Files,
    ]
}

fn responses() -> Vec<(u16, Response)> {
    let pose = Pose {
        position: [1.0, 2.0, 3.0],
        right: [1.0, 0.0, 0.0],
        up: [0.0, 1.0, 0.0],
        forward: [0.0, 0.0, 1.0],
    };
    vec![
        (
            kind::HELLO,
            Response::Hello(Hello {
                protocol: PROTOCOL_VERSION,
                module: "doctor_live 0.1.0".into(),
                edition_ok: true,
                edition: "PC 1.0.789.0".into(),
                writes: false,
            }),
        ),
        (
            kind::STATUS,
            Response::Status(Status {
                frame: 1234,
                state: LoopState::Game,
                player: true,
                sim_time_ms: Some(98_765),
                time_of_day: Some([13, 5, 59]),
            }),
        ),
        (
            kind::STATUS,
            Response::Status(Status {
                frame: 0,
                state: LoopState::Init,
                player: false,
                sim_time_ms: None,
                time_of_day: None,
            }),
        ),
        (kind::PLAYER, Response::Player(Some(pose))),
        (kind::PLAYER, Response::Player(None)),
        (
            kind::CAMERA,
            Response::Camera(Some(CameraView {
                pose,
                fov: Some(55.0),
            })),
        ),
        (kind::CAMERA, Response::Camera(None)),
        (
            kind::STREAMS,
            Response::Streams(vec![
                StreamInfo {
                    name: "gf2\\cuba".into(),
                },
                StreamInfo {
                    name: "gf2\\globals_global".into(),
                },
            ]),
        ),
        (
            kind::OBJECTS,
            Response::Objects(vec![ObjectInfo {
                guid: [3; 16],
                class: 0x3852_3FC3,
                class_name: "Entity".into(),
                position: [4.0, 5.0, 6.0],
                distance: 2.5,
            }]),
        ),
        (
            kind::TELEPORT,
            Response::Done(Done {
                count: 1,
                note: "player moved".into(),
            }),
        ),
        (
            kind::LUA,
            Response::Lua(LuaReply {
                ok: true,
                values: vec!["12.5".into(), "nil".into()],
            }),
        ),
        (kind::PEEK, Response::Peek(vec![1, 2, 3, 4])),
        (
            kind::TIME_OF_DAY,
            Response::TimeOfDay(Some(TimeOfDay {
                year: 1950,
                day: 0,
                hour: 17,
                minute: 50,
                second: 12,
                rate: 60.0,
                default_rate: 60.0,
            })),
        ),
        (kind::TIME_OF_DAY, Response::TimeOfDay(None)),
        (kind::WEATHER, Response::Weather(Weather::default())),
        (
            kind::WEATHER,
            Response::Weather(Weather {
                rain: Some(Rain {
                    flags: 5,
                    length: 120.0,
                    elapsed: 3.5,
                }),
                time: None,
            }),
        ),
        (
            kind::STREAM_QUEUE,
            Response::StreamQueue(StreamQueue {
                busy: true,
                requests: vec![StreamRequest {
                    name: "gf2\\cuba\\local".into(),
                    action: StreamAction::Unload,
                    id: 77,
                    state: 3,
                    done: false,
                    frames: 12,
                    expired: false,
                }],
            }),
        ),
        (
            kind::MONEY,
            Response::Money(Some(Money {
                cash: 1500.0,
                books: vec![0.0, 250.5],
            })),
        ),
        (kind::MONEY, Response::Money(None)),
        (
            kind::FAMILY,
            Response::Family(Family {
                families: vec![FamilyInfo {
                    hash: 0x0637_B907,
                    cash: 100.0,
                }],
                crew: vec![
                    CrewSlot {
                        slot: 0,
                        flags: 3,
                        rank: 3,
                        member: Some([9; 16]),
                        member_rank: 4,
                        specialties: 0x44,
                    },
                    CrewSlot {
                        slot: 6,
                        flags: 0,
                        rank: 5,
                        member: None,
                        member_rank: 0,
                        specialties: 0,
                    },
                ],
            }),
        ),
        (
            kind::MISSION,
            Response::Mission(Mission {
                current: Some(Checkpoint {
                    number: 737,
                    chapter: 60,
                    guid: [1; 16],
                }),
                checkpoints: vec![Checkpoint {
                    number: 739,
                    chapter: 60,
                    guid: [2; 16],
                }],
            }),
        ),
        (kind::MISSION, Response::Mission(Mission::default())),
        (
            kind::VARIABLES,
            Response::Variables(vec![Variable {
                hash: 0x001A_01D2,
                name_hash: 0x9381_3AB9,
                value: -1,
                kind: 1,
            }]),
        ),
        (
            kind::FILES,
            Response::Files(Files {
                hooked: true,
                logging: false,
                total: 3,
                redirects: vec![
                    FileUse {
                        from: "gf2/cuba.str".into(),
                        to: "Z:\\overlay\\gf2\\cuba.str".into(),
                        opens: 2,
                        failed: 0,
                    },
                    FileUse {
                        from: "cmd.txt".into(),
                        to: "_doctor_toolkit/overlay/cmd.txt".into(),
                        opens: 0,
                        failed: 1,
                    },
                ],
                passed: 1380,
            }),
        ),
        (kind::FILES, Response::Files(Files::default())),
        (
            kind::SET_MONEY,
            Response::Done(Done {
                count: 1,
                note: "money 1500".into(),
            }),
        ),
        (
            kind::EVENTS,
            Response::Events(EventBatch {
                next: 45,
                lost: 2,
                recording: crate::journal::mask::ALL,
                events: vec![
                    Event {
                        seq: 42,
                        frame: 900,
                        sim_time_ms: 12_000,
                        body: EventBody::Output {
                            source: OutputSource::Print,
                            text: "hello from a script".into(),
                            script: "@scripts/mission.lua".into(),
                            line: 17,
                        },
                    },
                    Event {
                        seq: 43,
                        frame: 901,
                        sim_time_ms: 12_050,
                        body: EventBody::LuaError {
                            status: 1,
                            message: "doctor_live:1: boom".into(),
                            chunk: "=doctor_live".into(),
                            line: 1,
                            traceback: vec!["doctor_live:1: in main chunk".into(), "[C]".into()],
                        },
                    },
                    Event {
                        seq: 44,
                        frame: 902,
                        sim_time_ms: 12_100,
                        body: EventBody::Message {
                            hash: 0x1234_5678,
                            data: 9,
                            forced: true,
                            listeners: 3,
                            receivers: vec![[5; 16]],
                            origin: MessageOrigin::Link,
                        },
                    },
                    Event {
                        seq: 45,
                        frame: 903,
                        sim_time_ms: 12_150,
                        body: EventBody::MessageCounts {
                            window_ms: 1000,
                            counts: vec![(0x1234_5678, 57), (7, 1)],
                        },
                    },
                ],
            }),
        ),
        (
            kind::DEBUG,
            Response::Debug(DebugState {
                rules: vec![BreakRule::message(5, None)],
                stop: Some(Stop {
                    reason: StopReason::Message {
                        hash: 5,
                        data: 0,
                        receivers: vec![[9; 16]],
                    },
                    place: StopPlace::Bus,
                    frame: 77,
                    sim_time_ms: 1500,
                    seq: 120,
                    held_ms: 3000,
                    resume_in_ms: 117_000,
                }),
                pending: false,
                hits: 1,
                hold_limit_s: 120,
                camera: None,
            }),
        ),
        (kind::DEBUG, Response::Debug(DebugState::default())),
        (
            kind::DEBUG,
            Response::Debug(DebugState {
                stop: Some(Stop {
                    reason: StopReason::Output {
                        text: "dt:a:1;".into(),
                        script: "a.lua".into(),
                        line: 9,
                    },
                    place: StopPlace::Print,
                    frame: 1,
                    sim_time_ms: 2,
                    seq: 3,
                    held_ms: 4,
                    resume_in_ms: 5,
                }),
                pending: true,
                camera: Some(CameraView { pose, fov: None }),
                ..DebugState::default()
            }),
        ),
        (
            kind::CONTROL,
            Response::Done(Done {
                count: 0,
                note: "paused".into(),
            }),
        ),
        (
            kind::WORLD,
            Response::World(World {
                total: 3,
                walked: 8_600,
                partial: false,
                classes: vec![(0x3852_3FC3, "Entity".into())],
                streams: vec![(4, "gf2\\cuba.str".into())],
                objects: vec![
                    WorldObject {
                        guid: [3; 16],
                        class: 0x3852_3FC3,
                        group: 4,
                        position: Some([1.0, 2.0, 3.0]),
                        flags: 0x2800_0000,
                        listening: true,
                    },
                    WorldObject {
                        guid: [4; 16],
                        class: 0,
                        group: 4,
                        position: None,
                        flags: 0,
                        listening: false,
                    },
                ],
            }),
        ),
        (
            kind::PLAYER,
            Response::error(ErrorCode::NotReady, "no player: the game is loading"),
        ),
    ]
}

#[test]
fn requests_round_trip() {
    for (i, request) in requests().into_iter().enumerate() {
        let frame = request.to_frame(i as u32);
        let (header, payload) = read_frame(&mut frame.as_slice()).unwrap();
        assert_eq!(header.version, PROTOCOL_VERSION);
        assert_eq!(header.id, i as u32);
        assert_eq!(header.kind, request.kind());
        assert_eq!(Request::decode(header.kind, &payload).unwrap(), request);
    }
}

#[test]
fn responses_round_trip() {
    for (i, (kind, response)) in responses().into_iter().enumerate() {
        let frame = response.to_frame(kind, i as u32);
        let (header, payload) = read_frame(&mut frame.as_slice()).unwrap();
        assert_eq!(header.id, i as u32);
        let back = Response::decode(header.kind, &payload).unwrap();
        assert_eq!(back, response, "kind {kind}");
    }
}

#[test]
fn writes_are_marked() {
    let writes: Vec<bool> = requests().iter().map(Request::writes).collect();
    assert_eq!(
        writes,
        [
            false, false, false, false, false, false, false, true, true, true, true, true, false,
            false, false, false, false, false, false, false, false, true, true, true, false, true,
            true, false, true, true, false, false, true, true, false, false, false, false, true,
            false
        ]
    );
}

#[test]
fn header_layout_is_fixed() {
    let bytes = Request::Status.to_frame(0x0102_0304);
    assert_eq!(
        bytes,
        [b'D', b'T', b'L', b'V', 5, 0, 2, 0, 4, 3, 2, 1, 0, 0, 0, 0]
    );
    // A client of stage 34 sends version 1, and gets its answer in it.
    let old = Request::Status.to_frame_of(1, 0x0102_0304);
    assert_eq!(old[4..6], [1, 0]);
    let reply = Response::Status(Status {
        frame: 1,
        state: LoopState::Game,
        player: true,
        sim_time_ms: None,
        time_of_day: None,
    })
    .to_frame_of(1, kind::STATUS, 7);
    let (header, payload) = read_frame(&mut reply.as_slice()).unwrap();
    assert_eq!(header.version, 1);
    assert!(matches!(
        Response::decode(header.kind, &payload),
        Ok(Response::Status(_))
    ));
}

#[test]
fn versions_and_their_requests() {
    assert!(supported(1) && supported(2) && supported(3) && supported(4) && supported(5));
    assert!(!supported(0) && !supported(6));
    assert_eq!(OLDEST_PROTOCOL_VERSION, 1);
    for r in requests() {
        let since = match r {
            Request::Events { .. } | Request::World { .. } => 2,
            Request::Break { .. }
            | Request::Control(_)
            | Request::Debug
            | Request::SetCamera { .. }
            | Request::Texture { .. } => 3,
            Request::TimeOfDay
            | Request::SetTime(_)
            | Request::Weather
            | Request::StreamLoad { .. }
            | Request::StreamUnload { .. }
            | Request::StreamQueue
            | Request::Money
            | Request::SetMoney(_)
            | Request::Family
            | Request::Mission
            | Request::Variables { .. }
            | Request::SetVariable { .. } => 4,
            Request::Files => 5,
            _ => 1,
        };
        assert_eq!(r.since(), since, "{r:?}");
        assert!(r.since() <= PROTOCOL_VERSION);
    }
}

#[test]
fn broken_journal_and_world_payloads_are_errors() {
    // An event of an unknown kind.
    let mut w = crate::wire::Writer::default();
    w.u64(2);
    w.u64(0);
    w.u8(7);
    w.u32(1);
    w.u64(1);
    w.u32(0);
    w.u32(0);
    w.u8(9);
    w.buf.extend_from_slice(&[0; 16]);
    assert_eq!(
        Response::decode(kind::EVENTS | REPLY, &w.buf),
        Err(DecodeError::BadValue("event"))
    );
    // A huge list of GUIDs in a filter.
    let mut bad = Request::World {
        filter: WorldFilter::default(),
        max: 1,
    }
    .encode();
    bad[1..5].copy_from_slice(&u32::MAX.to_le_bytes());
    assert_eq!(
        Request::decode(kind::WORLD, &bad),
        Err(DecodeError::Truncated)
    );
}

#[test]
fn other_versions_are_read_but_visible() {
    let mut bytes = Request::Status.to_frame(1);
    bytes[4] = 9;
    let (header, _) = read_frame(&mut bytes.as_slice()).unwrap();
    assert_eq!(header.version, 9);
}

#[test]
fn broken_frames_are_errors() {
    // Wrong magic.
    let mut bytes = Request::Status.to_frame(1);
    bytes[0] = b'X';
    let err = read_frame(&mut bytes.as_slice()).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    // A payload over the limit is refused before it is read.
    let mut head = Header {
        version: PROTOCOL_VERSION,
        kind: kind::LUA,
        id: 1,
        len: MAX_PAYLOAD as u32 + 1,
    }
    .to_bytes()
    .to_vec();
    head.extend_from_slice(&[0; 8]);
    assert!(read_frame(&mut head.as_slice()).is_err());
    // Cut short.
    let bytes = Request::Peek { address: 1, len: 2 }.to_frame(1);
    assert!(read_frame(&mut &bytes[..bytes.len() - 1]).is_err());
}

#[test]
fn broken_payloads_are_errors() {
    assert_eq!(
        Request::decode(kind::PEEK, &[1, 2, 3]),
        Err(DecodeError::Truncated)
    );
    assert_eq!(
        Request::decode(kind::STATUS, &[0]),
        Err(DecodeError::Trailing(1))
    );
    assert_eq!(Request::decode(99, &[]), Err(DecodeError::UnknownKind(99)));
    // Message tag out of range.
    assert_eq!(
        Request::decode(kind::SEND, &[5]),
        Err(DecodeError::BadValue("message"))
    );
    // A string that is not UTF-8.
    assert_eq!(
        Request::decode(kind::LUA, &[2, 0, 0xFF, 0xFE]),
        Err(DecodeError::BadUtf8)
    );
    // A reply kind without the reply bit.
    assert_eq!(
        Response::decode(kind::PLAYER, &[0]),
        Err(DecodeError::UnknownKind(kind::PLAYER))
    );
    // A list count larger than the payload can hold.
    assert_eq!(
        Response::decode(kind::OBJECTS | REPLY, &[0xFF, 0xFF, 0xFF, 0x7F]),
        Err(DecodeError::Truncated)
    );
    // A bool that is neither 0 nor 1.
    assert_eq!(
        Response::decode(kind::PLAYER | REPLY, &[2]),
        Err(DecodeError::BadValue("bool"))
    );
}

#[test]
fn long_strings_are_cut_on_a_character_boundary() {
    let code = "ж".repeat(40_000); // 80 000 bytes
    let payload = Request::Lua { code }.encode();
    let back = Request::decode(kind::LUA, &payload).unwrap();
    let Request::Lua { code } = back else {
        panic!("not Lua")
    };
    assert_eq!(code.len(), 65_534);
    assert!(code.chars().all(|c| c == 'ж'));
}

#[test]
fn message_ids() {
    assert_eq!(MessageId::parse("0x1A2b"), MessageId::Hash(0x1A2B));
    assert_eq!(
        MessageId::parse(" doctor_ping "),
        MessageId::Name("doctor_ping".into())
    );
    assert_eq!(
        MessageId::parse("0xnothex"),
        MessageId::Name("0xnothex".into())
    );
    assert_eq!(
        MessageId::Name("Doctor_Ping".into()).hash(),
        crate::name_hash("doctor_ping")
    );
}

#[test]
fn yaw_of_a_pose() {
    let pose = Pose {
        position: [0.0; 3],
        right: [0.0; 3],
        up: [0.0, 1.0, 0.0],
        forward: [1.0, 0.0, 0.0],
    };
    assert!((pose.yaw() - std::f32::consts::FRAC_PI_2).abs() < 1e-6);
}

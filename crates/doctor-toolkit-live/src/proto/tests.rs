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
            false, false, false, false, false, false, false, true, true, true, true, true, false
        ]
    );
}

#[test]
fn header_layout_is_fixed() {
    let bytes = Request::Status.to_frame(0x0102_0304);
    assert_eq!(
        bytes,
        [b'D', b'T', b'L', b'V', 1, 0, 2, 0, 4, 3, 2, 1, 0, 0, 0, 0]
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

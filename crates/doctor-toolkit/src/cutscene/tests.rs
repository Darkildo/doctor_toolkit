use super::*;
use crate::resource::trinity::{CurveKey, FixedText, PositionKey, RotationKey, SeqBlock};

fn guid() -> Guid {
    Guid(*b"\x01\x02\x03\x04\x05\x06\x07\x08DOCTORTK")
}

fn shot(start: f32) -> Shot {
    let actor = guid();
    let commands = vec![
        Command::ActorCreate {
            actor,
            name: FixedText::new("igc99_hero"),
            flags: [0; 3],
            position: [1.0, 0.0, 0.0],
            reserved: 0,
        },
        Command::CameraCreate {
            camera: 0,
            flags: 0,
            aspect: 1.5,
        },
        Command::Wait { seconds: 0.0 },
        Command::CameraCurves {
            camera: 0,
            curves: std::array::from_fn(|i| {
                if i == 0 {
                    vec![CurveKey::flat(0.0, 0.0), CurveKey::flat(2.0, 10.0)]
                } else {
                    vec![CurveKey::flat(0.0, 0.0)]
                }
            }),
        },
        Command::CameraFov {
            camera: 0,
            keys: vec![CurveKey::flat(0.0, 40.0)],
        },
        Command::Wait { seconds: 1.0 },
        Command::ActorTransform {
            actor,
            positions: vec![
                PositionKey {
                    time: 0.0,
                    position: [0.0, 0.0, 0.0],
                },
                PositionKey {
                    time: 1.0,
                    position: [4.0, 0.0, 0.0],
                },
            ],
            rotations: vec![
                RotationKey {
                    time: 0.0,
                    rotation: [0.0, 0.0, 0.0, 1.0],
                },
                RotationKey {
                    time: 1.0,
                    rotation: [0.0, 0.0, 0.0, 1.0],
                },
            ],
        },
        Command::ActorAnim {
            actor,
            clip: FixedText::new("igc99_hero_s01"),
            speed: 1.0,
            params: [0, 0, 8, 1, 0],
        },
        Command::Wait { seconds: 1.0 },
    ];
    Shot {
        resource: 0,
        id: 1,
        seq: SeqMaster {
            path: FixedText::new("x:/assets/igcs/igc99/source/igc99_s01.xml"),
            blocks: vec![SeqBlock {
                version: 4,
                hash: 0,
                transform: [0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0],
                name: FixedText::new("trSeq_igc99_s01"),
                duration: 2.0,
                commands,
            }],
        },
        start,
        stream: None,
        stream_name: String::new(),
        actors: Vec::new(),
        clips: vec![ClipInfo {
            bank: 0,
            clip: 0,
            hash: sdbm(b"igc99_hero_s01"),
            name: Some("igc99_hero_s01".into()),
            frames: 15,
        }],
    }
}

#[test]
fn camera_follows_the_latest_path() {
    let s = shot(0.0);
    let c = camera_state(&s, 1.0).unwrap();
    assert!((c.values[0] - 5.0).abs() < 1e-4);
    assert_eq!(c.fov, 40.0);
    assert_eq!(c.aspect, 1.5);
    let m = c.matrix();
    assert!((m[12] - 5.0).abs() < 1e-4);
    assert_eq!(s.seq.scene_name(), Some("igc99"));
}

#[test]
fn actors_are_placed_and_animated() {
    let s = shot(0.0);
    let before = actor_state(&s, &guid(), 0.5).unwrap();
    assert_eq!(before.position, [1.0, 0.0, 0.0]);
    assert!(before.clip.is_none());
    let during = actor_state(&s, &guid(), 1.5).unwrap();
    assert!((during.position[0] - 2.0).abs() < 1e-5);
    let (clip, at) = during.clip.unwrap();
    assert_eq!(clip, "igc99_hero_s01");
    assert!((at - 0.5).abs() < 1e-6);
    assert!(actor_state(&s, &Guid::default(), 1.0).is_none());
}

#[test]
fn spans_on_the_time_line() {
    let s = shot(10.0);
    let times = s.block().times();
    let anim = s
        .block()
        .commands
        .iter()
        .position(|c| matches!(c, Command::ActorAnim { .. }))
        .unwrap();
    let (start, length) = command_span(&s, anim, times[anim]);
    assert!((start - 11.0).abs() < 1e-6);
    assert!((length - 0.5).abs() < 1e-6);
    let curves = 3;
    assert_eq!(command_span(&s, curves, times[curves]), (10.0, 2.0));
    assert_eq!(s.end(), 12.0);
}

#[test]
fn cutscene_finds_the_shot_at_a_time() {
    let cutscene = Cutscene {
        stream: 0,
        stream_name: String::new(),
        name: "igc99".into(),
        sequence: None,
        shots: vec![shot(0.0), shot(2.0)],
        audio: Vec::new(),
    };
    assert_eq!(cutscene.duration(), 4.0);
    assert_eq!(cutscene.shot_at(0.5), Some(0));
    assert_eq!(cutscene.shot_at(2.5), Some(1));
    assert_eq!(cutscene.shot_at(9.0), Some(1));
    assert_eq!(cutscene.camera_at(3.0).unwrap().shot, 1);
}

#[test]
fn the_mini_game_cutscene_loads_and_takes_a_camera_key() {
    use crate::testing;
    let dir = tempfile::tempdir().unwrap();
    testing::write_game(dir.path()).unwrap();
    testing::write_cutscene(dir.path()).unwrap();
    let game = GameInstance::open(dir.path()).unwrap();
    let stream = game.find_stream(testing::CUTSCENE).unwrap();
    let c = Cutscene::load(&Source::new(&game, None), stream, None).unwrap();
    assert_eq!(c.shots.len(), 1);
    assert_eq!(c.name, "igc99");
    let camera = c.camera_at(1.0).unwrap();
    assert!((camera.values[0] - 5.0).abs() < 1e-4);
    // A key at 1 s, written and read back.
    let mut seq = c.shots[0].seq.clone();
    let edit = edit::set_camera(&mut seq, 1.0, [5.0, 3.0, 8.0, 0.0, 0.0, 0.0]).unwrap();
    assert_eq!(edit, edit::KeyEdit::Inserted);
    let back = SeqMaster::parse(&seq.to_bytes().unwrap()).unwrap();
    assert_eq!(back, seq);
    let mut shot = c.shots[0].clone();
    shot.seq = back;
    let at = camera_state(&shot, 1.0).unwrap();
    assert!((at.values[1] - 3.0).abs() < 1e-4);
    // The sound: the mini game has none.
    assert!(
        sound::mix(&Source::new(&game, None), &c, 0.0, 1.0)
            .unwrap()
            .is_none()
    );
}

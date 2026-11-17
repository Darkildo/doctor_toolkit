//! A server that answers like the module in the game: for the tests of the
//! library, the command line and the editor, and to try the live panel
//! without the game (`doctor-toolkit live fake`).
//!
//! It keeps a small world — a player, a camera, a few objects, logic
//! objects without a position — and does what the module does with it:
//! teleport moves the player, a message counts the listeners that would get
//! it, Lua answers a few expressions. Its journal records `print(…)` and
//! failing Lua code, and the messages it delivers; it speaks protocol 1 and
//! 2 like the module (or only 1, as a module of stage 34:
//! [`FakeWorld::protocol`]). Protocol 3 (stage 35): breakpoints stop the
//! fake game (its frame and time stand still), pause and step, the free
//! camera, textures by name; [`FakeWorld::activity`] makes the game's own
//! traffic for trying the editor (`live fake --activity`). Protocol 4
//! (stage 48): a clock, a stream streamer that loads one request at a time
//! (it is done at the next look at the queue), money, a crew, checkpoints
//! and variables of the story. Protocol 5 (stage 46): the files of the
//! configuration's `redirect` lines, each opened once by the fake game.
//! Protocol 6 (stage 47): objects with a matrix move, turn, hide and show
//! and the journal of these edits undoes them, as in the module
//! ([`FakeEntity`]). Protocol 7 (stage 58): a profile made up from the
//! frame count ([`fake_profile`]): a game at about 30 frames a second with
//! a slow pass now and then, reads of three threads, a few textures.

use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use doctor_toolkit_live::debug::{
    BreakOn, BreakRule, Control, DebugState, HOLD_DEFAULT_S, Stop, StopPlace, StopReason,
};
use doctor_toolkit_live::journal::{EventBody, MessageOrigin, OutputSource, Ring, mask};
use doctor_toolkit_live::state::{STREAM_LOADED, stream_name};
use doctor_toolkit_live::{
    CameraView, Checkpoint, CrewSlot, Done, ErrorCode, Family, FamilyInfo, Guid, Hello, LoopState,
    LuaReply, Mission, Money, MoneyChange, ObjectInfo, PROTOCOL_VERSION, Pose, Request, Response,
    Status, StreamAction, StreamInfo, StreamQueue, StreamRequest, TimeOfDay, Variable, Weather,
    World, WorldObject, read_frame,
};

use super::Config;

use doctor_toolkit_live::edit::{
    EditJournal, EditKind, EditValue, Matrix, ObjectState, transform_note,
};

/// An object of the fake game the link can edit (protocol 6).
#[derive(Debug, Clone, PartialEq)]
pub struct FakeEntity {
    pub guid: Guid,
    /// `None`: logic without a matrix (only its listener can be switched).
    pub matrix: Option<Matrix>,
    /// The count of deactivations: 0 in the world.
    pub inactive: u8,
    pub listening: bool,
}

/// The world of the fake game.
#[derive(Debug, Clone)]
pub struct FakeWorld {
    pub frame: u32,
    pub state: LoopState,
    pub player: Option<Pose>,
    pub camera: Option<CameraView>,
    pub sim_time_ms: u32,
    pub streams: Vec<String>,
    /// Objects with the messages they listen to.
    pub objects: Vec<(ObjectInfo, Vec<u32>)>,
    /// Objects without a world matrix (logic), with their class name and
    /// the messages they listen to; all objects are of group 1
    /// (`gf2\cuba.str`).
    pub logic: Vec<(Guid, String, Vec<u32>)>,
    /// The journal.
    pub journal: Ring,
    /// Newest protocol it speaks: 2, or 1 like a module of stage 34.
    pub protocol: u16,
    /// Messages delivered: hash and how many listeners got it.
    pub sent: Vec<(u32, u32)>,
    /// The build is not the known one.
    pub wrong_edition: bool,
    /// Breakpoints, stop, free camera (protocol 3).
    pub debug: DebugState,
    /// Loaded textures: name hash and the bytes their levels take.
    pub textures: Vec<(u32, u64)>,
    /// Textures replaced: name hash and bytes.
    pub replaced: Vec<(u32, usize)>,
    /// The clock (protocol 4).
    pub time: TimeOfDay,
    /// The player's money.
    pub money: Money,
    /// The player's crew.
    pub crew: Vec<CrewSlot>,
    /// Current checkpoint and those known.
    pub mission: Mission,
    /// Variables of the story.
    pub variables: Vec<Variable>,
    /// Stream requests of the link.
    pub stream_requests: Vec<StreamRequest>,
    /// Objects the link can edit: the positioned ones and the logic
    /// (protocol 6).
    pub entities: Vec<FakeEntity>,
    /// The journal of edits.
    pub edits: EditJournal,
    /// The frame of the last reset of the profile (protocol 7).
    pub profile_reset: u32,
}

/// A variable of the fake game: `doctor_fake_flag`, 0 at the start.
pub const FAKE_VARIABLE: &str = "doctor_fake_flag";

/// The texture of the fake game: 4 × 4 A8R8G8B8, three levels (84 bytes).
pub const FAKE_TEXTURE: &str = "doctor_fake_texture";

impl Default for FakeWorld {
    fn default() -> Self {
        let pose = Pose {
            position: [283.9, -0.07, 371.8],
            right: [1.0, 0.0, 0.0],
            up: [0.0, 1.0, 0.0],
            forward: [0.0, 0.0, 1.0],
        };
        let object = |guid: u8, class: &str, position: [f32; 3], listens: &[&str]| {
            let d = ((position[0] - pose.position[0]).powi(2)
                + (position[1] - pose.position[1]).powi(2)
                + (position[2] - pose.position[2]).powi(2))
            .sqrt();
            (
                ObjectInfo {
                    guid: [guid; 16],
                    class: doctor_toolkit_live::name_hash(class),
                    class_name: class.to_owned(),
                    position,
                    distance: d,
                },
                listens
                    .iter()
                    .map(|m| doctor_toolkit_live::name_hash(m))
                    .collect(),
            )
        };
        Self {
            frame: 100,
            state: LoopState::Game,
            player: Some(pose),
            camera: Some(CameraView {
                pose: Pose {
                    position: [283.9, 1.6, 367.3],
                    ..pose
                },
                fov: Some(55.0),
            }),
            sim_time_ms: 12_345,
            streams: vec!["gf2\\cuba".into(), "gf2\\globals_global".into()],
            objects: vec![
                object(1, "VariableSwitch", [285.0, 0.0, 372.0], &["doctor_ping"]),
                object(2, "PlayerSpawn", [283.9, -0.07, 371.8], &[]),
                object(3, "MessageRelay", [300.0, 0.0, 400.0], &["doctor_ping"]),
            ],
            logic: vec![
                (
                    [4; 16],
                    "VariableSwitch".into(),
                    vec![doctor_toolkit_live::name_hash("doctor_ping")],
                ),
                ([5; 16], "MessageRelay".into(), Vec::new()),
            ],
            journal: Ring::default(),
            protocol: PROTOCOL_VERSION,
            sent: Vec::new(),
            wrong_edition: false,
            debug: DebugState {
                hold_limit_s: HOLD_DEFAULT_S,
                ..DebugState::default()
            },
            textures: vec![(doctor_toolkit_live::name_hash(FAKE_TEXTURE), 84)],
            replaced: Vec::new(),
            time: TimeOfDay {
                year: 1950,
                day: 0,
                hour: 12,
                minute: 0,
                second: 0,
                rate: 60.0,
                default_rate: 60.0,
            },
            money: Money {
                cash: 0.0,
                books: vec![0.0; 11],
            },
            crew: (0..7u8)
                .map(|i| CrewSlot {
                    slot: i,
                    flags: if i == 0 { 3 } else { 0 },
                    rank: [3, 3, 3, 3, 4, 4, 5][i as usize],
                    member: (i == 0).then_some([4; 16]),
                    member_rank: if i == 0 { 3 } else { 0 },
                    specialties: if i == 0 { 0x40 } else { 0 },
                })
                .collect(),
            mission: Mission {
                current: None,
                checkpoints: vec![Checkpoint {
                    number: 737,
                    chapter: 60,
                    guid: [5; 16],
                }],
            },
            variables: vec![Variable {
                hash: 0x0CA4_32A1,
                name_hash: doctor_toolkit_live::name_hash(FAKE_VARIABLE),
                value: 0,
                kind: 1,
            }],
            stream_requests: Vec::new(),
            entities: [1u8, 2, 3]
                .iter()
                .zip([
                    [285.0, 0.0, 372.0],
                    [283.9, -0.07, 371.8],
                    [300.0, 0.0, 400.0],
                ])
                .map(|(&g, p)| FakeEntity {
                    guid: [g; 16],
                    matrix: Some([
                        1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, p[0], p[1],
                        p[2], 1.0,
                    ]),
                    inactive: 0,
                    listening: true,
                })
                .chain([4u8, 5].iter().map(|&g| FakeEntity {
                    guid: [g; 16],
                    matrix: None,
                    inactive: 0,
                    listening: true,
                }))
                .collect(),
            edits: EditJournal::new(),
            profile_reset: 0,
        }
    }
}

impl FakeWorld {
    /// The state of an edited object, as the module reports it.
    pub fn object_state(&self, guid: &Guid) -> Option<ObjectState> {
        let e = self.entities.iter().find(|e| &e.guid == guid)?;
        let (class, class_name) = self
            .objects
            .iter()
            .map(|(o, _)| (o.guid, o.class, o.class_name.clone()))
            .chain(
                self.logic
                    .iter()
                    .map(|(g, c, _)| (*g, doctor_toolkit_live::name_hash(c), c.clone())),
            )
            .find(|(g, _, _)| g == guid)
            .map(|(_, c, n)| (c, n))
            .unwrap_or_default();
        Some(ObjectState {
            guid: *guid,
            class,
            class_name,
            matrix: e.matrix,
            inactive: e.matrix.map(|_| e.inactive),
            has_model: e.matrix.is_some(),
            listening: e.listening,
            hidden_by_link: self.edits.hidden(guid),
            unsupported: if e.matrix.is_some() {
                String::new()
            } else {
                "no world matrix at +0xD0".into()
            },
        })
    }

    /// Gives an object a matrix; its position follows in the list of
    /// objects.
    fn place(&mut self, guid: &Guid, m: Matrix) {
        if let Some(e) = self.entities.iter_mut().find(|e| &e.guid == guid) {
            e.matrix = Some(m);
        }
        if let Some((o, _)) = self.objects.iter_mut().find(|(o, _)| &o.guid == guid) {
            o.position = [m[12], m[13], m[14]];
        }
    }

    /// `ObjectTransform`, `ObjectEnable`, `ObjectUndo` (protocol 6).
    fn edit(&mut self, request: Request) -> Response {
        let missing = |g: &Guid| {
            Response::error(
                ErrorCode::Failed,
                format!(
                    "no object {} in the game (its stream is not loaded, or it was never made)",
                    doctor_toolkit_live::guid_hex(g)
                ),
            )
        };
        if self.state != LoopState::Game {
            return Response::error(ErrorCode::NotReady, "not in a level");
        }
        match request {
            Request::ObjectTransform { guid, matrix } => {
                if !doctor_toolkit_live::is_world_matrix(&matrix, true) {
                    return Response::error(ErrorCode::BadRequest, "not a world matrix");
                }
                let Some(state) = self.object_state(&guid) else {
                    return missing(&guid);
                };
                let Some(before) = state.matrix else {
                    return Response::error(
                        ErrorCode::Failed,
                        format!("not changed: {}", state.unsupported),
                    );
                };
                self.place(&guid, matrix);
                let note = transform_note(&before, &matrix);
                Response::ObjectEdit(self.edits.record(
                    guid,
                    EditKind::Transform,
                    EditValue::Matrix(before),
                    EditValue::Matrix(matrix),
                    note,
                ))
            }
            Request::ObjectEnable {
                guid,
                enabled,
                logic,
            } => {
                let held = self.edits.hidden(&guid);
                let Some(e) = self.entities.iter_mut().find(|e| e.guid == guid) else {
                    return missing(&guid);
                };
                if logic {
                    if e.listening == enabled {
                        return Response::error(ErrorCode::BadRequest, "its logic is so already");
                    }
                    e.listening = enabled;
                    return Response::ObjectEdit(self.edits.record(
                        guid,
                        EditKind::Listening,
                        EditValue::Flag(!enabled),
                        EditValue::Flag(enabled),
                        format!("logic {}", if enabled { "on" } else { "off" }),
                    ));
                }
                if e.matrix.is_none() {
                    return Response::error(
                        ErrorCode::Failed,
                        "not changed: no world matrix at +0xD0",
                    );
                }
                if enabled != held {
                    return Response::error(
                        ErrorCode::BadRequest,
                        if enabled {
                            "it is in the world already"
                        } else {
                            "the link hid it already"
                        },
                    );
                }
                if enabled {
                    e.inactive = e.inactive.saturating_sub(1);
                } else {
                    e.inactive += 1;
                }
                Response::ObjectEdit(self.edits.record(
                    guid,
                    EditKind::Shown,
                    EditValue::Flag(!enabled),
                    EditValue::Flag(enabled),
                    if enabled { "shown" } else { "hidden" }.into(),
                ))
            }
            Request::ObjectUndo { guid } => {
                let Some(edit) = self.edits.last(guid.as_ref()).cloned() else {
                    return Response::error(ErrorCode::BadRequest, "nothing to undo");
                };
                if !self.entities.iter().any(|e| e.guid == edit.guid) {
                    return missing(&edit.guid);
                }
                match (edit.kind, edit.before) {
                    (EditKind::Transform, EditValue::Matrix(m)) => self.place(&edit.guid, m),
                    (kind, EditValue::Flag(on)) => {
                        if let Some(e) = self.entities.iter_mut().find(|e| e.guid == edit.guid) {
                            if kind == EditKind::Listening {
                                e.listening = on;
                            } else if on {
                                e.inactive = e.inactive.saturating_sub(1);
                            } else {
                                e.inactive += 1;
                            }
                        }
                    }
                    _ => {}
                }
                self.edits.take(edit.id);
                Response::ObjectEdit(doctor_toolkit_live::ObjectEdit {
                    id: edit.id,
                    guid: edit.guid,
                    kind: edit.kind,
                    before: edit.after,
                    after: edit.before,
                    note: format!("undid #{}", edit.id),
                })
            }
            _ => Response::error(ErrorCode::BadRequest, "not an edit"),
        }
    }

    /// Requests of the streamer are done at the next look at the queue.
    fn stream_tick(&mut self) {
        for r in self.stream_requests.iter_mut().filter(|r| !r.done) {
            r.done = true;
            r.frames += 1;
            match r.action {
                StreamAction::Load => {
                    r.state = STREAM_LOADED;
                    if !self.streams.contains(&r.name) {
                        self.streams.push(r.name.clone());
                    }
                }
                StreamAction::Unload => {
                    r.state = 0;
                    self.streams.retain(|s| s != &r.name);
                }
            }
        }
    }

    fn stream_request(&mut self, name: &str, action: StreamAction, force: bool) -> Response {
        let name = stream_name(name);
        if let Some(open) = self.stream_requests.iter().find(|r| !r.done) {
            return Response::error(
                ErrorCode::NotReady,
                format!(
                    "the stream {} of the link is not done yet; one request at a time",
                    open.name
                ),
            );
        }
        let loaded = self.streams.contains(&name);
        match action {
            StreamAction::Load if loaded => {
                return Response::Done(Done {
                    count: 0,
                    note: format!("{name} is loaded already"),
                });
            }
            StreamAction::Load if !name.starts_with("gf2\\") => {
                return Response::error(
                    ErrorCode::Failed,
                    format!("the game knows no stream {name}"),
                );
            }
            StreamAction::Unload if !loaded => {
                return Response::error(ErrorCode::Failed, format!("{name} is not loaded"));
            }
            StreamAction::Unload
                if !force
                    && !self
                        .stream_requests
                        .iter()
                        .any(|r| r.name == name && r.action == StreamAction::Load) =>
            {
                return Response::error(
                    ErrorCode::Denied,
                    format!("{name} was not loaded by the link"),
                );
            }
            _ => {}
        }
        self.stream_requests.push(StreamRequest {
            name: name.clone(),
            action,
            id: 0x40 + self.stream_requests.len() as u32,
            state: if loaded { STREAM_LOADED } else { 0 },
            done: false,
            frames: 0,
            expired: false,
        });
        Response::Done(Done {
            count: 1,
            note: format!(
                "{} {name}",
                if action == StreamAction::Load {
                    "asked for"
                } else {
                    "unloading"
                }
            ),
        })
    }
}

/// A running fake module; stops when dropped.
pub struct FakeGame {
    pub port: u16,
    pub world: Arc<Mutex<FakeWorld>>,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl FakeGame {
    /// Starts listening on a free port of 127.0.0.1 (`config.port` is
    /// ignored; the token and `writes` count).
    pub fn start(config: Config) -> std::io::Result<Self> {
        Self::start_on(0, config)
    }

    /// Starts on `port` (0: any free one).
    pub fn start_on(port: u16, config: Config) -> std::io::Result<Self> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, port))?;
        let port = listener.local_addr()?.port();
        let world = Arc::new(Mutex::new(FakeWorld::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let thread = {
            let world = world.clone();
            let stop = stop.clone();
            std::thread::spawn(move || {
                for stream in listener.incoming() {
                    if stop.load(Ordering::Relaxed) {
                        break;
                    }
                    let Ok(stream) = stream else { continue };
                    let world = world.clone();
                    let config = config.clone();
                    std::thread::spawn(move || serve(stream, &config, &world));
                }
            })
        };
        Ok(Self {
            port,
            world,
            stop,
            thread: Some(thread),
        })
    }

    pub fn endpoint(&self, token: &str) -> super::Endpoint {
        super::Endpoint {
            port: self.port,
            token: token.to_owned(),
        }
    }
}

impl Drop for FakeGame {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        let _ = TcpStream::connect((Ipv4Addr::LOCALHOST, self.port));
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn serve(mut stream: TcpStream, config: &Config, world: &Mutex<FakeWorld>) {
    let mut greeted = false;
    while let Ok((header, payload)) = read_frame(&mut stream) {
        let newest = world.lock().unwrap_or_else(|e| e.into_inner()).protocol;
        let reply = if !(1..=newest).contains(&header.version) {
            let r = Response::error(ErrorCode::Version, "other protocol version");
            let _ = std::io::Write::write_all(
                &mut stream,
                &r.to_frame_of(newest, header.kind, header.id),
            );
            return;
        } else {
            match Request::decode(header.kind, &payload) {
                Err(e) => Response::error(ErrorCode::BadRequest, e.to_string()),
                Ok(r) if r.since() > header.version => {
                    Response::error(ErrorCode::Unsupported, "newer request than the frame")
                }
                Ok(Request::Hello { token, .. }) => {
                    if !config.token.is_empty() && token != config.token {
                        Response::error(ErrorCode::Denied, "wrong token")
                    } else {
                        greeted = true;
                        let w = world.lock().unwrap_or_else(|e| e.into_inner());
                        Response::Hello(Hello {
                            protocol: w.protocol,
                            module: "doctor_live fake".into(),
                            edition_ok: !w.wrong_edition,
                            edition: if w.wrong_edition {
                                "not PC 1.0.789.0: link time 0x12345678".into()
                            } else {
                                doctor_toolkit_live::edition::NAME.into()
                            },
                            writes: config.writes,
                        })
                    }
                }
                Ok(_) if !greeted => Response::error(ErrorCode::Denied, "send Hello first"),
                Ok(r) => answer(
                    r,
                    config,
                    header.version,
                    &mut world.lock().unwrap_or_else(|e| e.into_inner()),
                ),
            }
        };
        if std::io::Write::write_all(
            &mut stream,
            &reply.to_frame_of(header.version, header.kind, header.id),
        )
        .is_err()
        {
            return;
        }
    }
}

impl FakeWorld {
    /// Records an event in the journal, as the module's hooks do.
    pub fn record(&mut self, body: EventBody) {
        let second = u64::from(self.frame / 30);
        self.journal
            .push(second, self.frame, self.sim_time_ms, body);
    }

    /// Whether the game stands at a breakpoint.
    pub fn stopped(&self) -> bool {
        self.debug.stop.is_some()
    }

    fn stop(&mut self, reason: StopReason, place: StopPlace) {
        self.debug.hits += 1;
        self.debug.stop = Some(Stop {
            reason,
            place,
            frame: self.frame,
            sim_time_ms: self.sim_time_ms,
            seq: self.journal.next_seq(),
            held_ms: 0,
            resume_in_ms: u32::from(self.debug.hold_limit_s) * 1000,
        });
    }

    /// The first rule `hit` matches; one-shot rules go away.
    fn take_hit(&mut self, hit: impl Fn(&BreakRule) -> bool) -> bool {
        let Some(i) = self.debug.rules.iter().position(hit) else {
            return false;
        };
        if self.debug.rules[i].once {
            self.debug.rules.remove(i);
        }
        true
    }

    /// The game's bus delivers `hash` to the objects that listen to it
    /// (recorded, and stopped at by a breakpoint, like the module does):
    /// `origin` is who sent it, `place` where a hit stops.
    pub fn deliver(&mut self, hash: u32, origin: MessageOrigin, place: StopPlace) -> u32 {
        let receivers: Vec<Guid> = self
            .objects
            .iter()
            .map(|(o, listens)| (o.guid, listens))
            .chain(self.logic.iter().map(|(g, _, listens)| (*g, listens)))
            .filter(|(_, listens)| listens.contains(&hash))
            .map(|(g, _)| g)
            .collect();
        if !self.stopped() && self.take_hit(|r| r.hit_by_message(hash, &mut || receivers.clone())) {
            self.stop(
                StopReason::Message {
                    hash,
                    data: 0,
                    receivers: receivers.clone(),
                },
                place,
            );
        }
        let count = receivers.len() as u32;
        if count > 0 {
            self.record(EventBody::Message {
                hash,
                data: 0,
                forced: false,
                listeners: count,
                receivers,
                origin,
                from: None,
            });
        }
        count
    }

    /// The game prints `text` from `script` (Lua `print`).
    pub fn print(&mut self, text: &str, script: &str, line: i32) {
        self.record(EventBody::Output {
            source: OutputSource::Print,
            text: text.to_owned(),
            script: script.to_owned(),
            line,
        });
        if !self.stopped() && self.take_hit(|r| r.hit_by_output(text)) {
            self.stop(
                StopReason::Output {
                    text: text.to_owned(),
                    script: script.to_owned(),
                    line,
                },
                StopPlace::Print,
            );
        }
    }

    /// One beat of the game's own traffic (`live fake --activity`): a
    /// trigger sends `doctor_ping` to two objects, a traced node of the
    /// visual script `doctor_demo` prints, and a message of the engine goes
    /// out sixty times — recorded as a count (the rate limit per message).
    /// Nothing happens while the game stands.
    pub fn activity(&mut self, beat: u64) {
        if self.stopped() {
            return;
        }
        self.frame += 30;
        self.sim_time_ms += 1000;
        self.deliver(
            doctor_toolkit_live::name_hash("doctor_ping"),
            MessageOrigin::Game,
            StopPlace::Bus,
        );
        self.print(
            &format!("dt:doctor_demo:{};", 2 + beat % 2),
            "doctor_demo.lua",
            40,
        );
        self.record(EventBody::MessageCounts {
            window_ms: 1000,
            counts: vec![(doctor_toolkit_live::name_hash("iMsgRunningTick"), 60)],
        });
    }
}

fn answer(request: Request, config: &Config, version: u16, w: &mut FakeWorld) -> Response {
    if w.wrong_edition {
        return Response::error(ErrorCode::Edition, "not PC 1.0.789.0");
    }
    if request.writes() && !config.writes {
        return Response::error(
            ErrorCode::Denied,
            "writes are switched off in doctor_live.cfg",
        );
    }
    if w.stopped() {
        let in_frame = w
            .debug
            .stop
            .as_ref()
            .is_some_and(|s| s.place.allows_writes());
        if request.writes() && !in_frame {
            return Response::error(
                ErrorCode::NotReady,
                "the game stands at a breakpoint inside its code",
            );
        }
    } else {
        w.frame += 1;
        w.sim_time_ms += 50;
    }
    match request {
        Request::Hello { .. } => unreachable!("handled by serve"),
        Request::Status => Response::Status(Status {
            frame: w.frame,
            state: w.state,
            player: w.player.is_some(),
            sim_time_ms: Some(w.sim_time_ms),
            time_of_day: Some([w.time.hour, w.time.minute, w.time.second]),
        }),
        Request::Player => Response::Player(w.player),
        Request::Camera => Response::Camera(w.debug.camera.or(w.camera)),
        Request::Streams => Response::Streams(
            w.streams
                .iter()
                .map(|s| StreamInfo { name: s.clone() })
                .collect(),
        ),
        Request::Objects {
            center,
            radius,
            class,
            max,
        } => {
            let Some(c) = center.or(w.player.map(|p| p.position)) else {
                return Response::error(ErrorCode::NotReady, "no player and no centre");
            };
            let mut list: Vec<ObjectInfo> = w
                .objects
                .iter()
                .map(|(o, _)| {
                    let mut o = o.clone();
                    o.distance = ((o.position[0] - c[0]).powi(2)
                        + (o.position[1] - c[1]).powi(2)
                        + (o.position[2] - c[2]).powi(2))
                    .sqrt();
                    o
                })
                .filter(|o| o.distance <= radius && class.is_none_or(|k| k == o.class))
                .collect();
            list.sort_by(|a, b| a.distance.total_cmp(&b.distance));
            list.truncate(max as usize);
            Response::Objects(list)
        }
        Request::Teleport { position, yaw } => {
            let Some(p) = w.player.as_mut() else {
                return Response::error(ErrorCode::NotReady, "no player");
            };
            let from = p.position;
            p.position = position;
            if let Some(a) = yaw {
                p.forward = [a.sin(), 0.0, a.cos()];
                p.right = [a.cos(), 0.0, -a.sin()];
            }
            Response::Done(Done {
                count: 1,
                note: format!(
                    "teleport from ({:.2}, {:.2}, {:.2}) to ({:.2}, {:.2}, {:.2})",
                    from[0], from[1], from[2], position[0], position[1], position[2]
                ),
            })
        }
        Request::Send {
            message,
            target: None,
        } => {
            // A message the link sends stops in the next frame (the module
            // must answer the request first).
            let hash = message.hash();
            let count = w.deliver(hash, MessageOrigin::Link, StopPlace::Frame);
            w.sent.push((hash, count));
            Response::Done(Done {
                count,
                note: format!("{count} listener(s)"),
            })
        }
        Request::Send { message, target } => {
            let hash = message.hash();
            let receivers: Vec<Guid> = w
                .objects
                .iter()
                .map(|(o, listens)| (o.guid, listens))
                .chain(w.logic.iter().map(|(g, _, listens)| (*g, listens)))
                .filter(|(g, listens)| listens.contains(&hash) && target.is_none_or(|t| t == *g))
                .map(|(g, _)| g)
                .collect();
            let count = receivers.len() as u32;
            w.sent.push((hash, count));
            if count > 0 {
                w.record(EventBody::Message {
                    hash,
                    data: 0,
                    forced: false,
                    listeners: count,
                    receivers,
                    origin: MessageOrigin::Link,
                    from: None,
                });
            }
            Response::Done(Done {
                count,
                note: format!("{count} listener(s)"),
            })
        }
        Request::Lua { code } => {
            let code = code.trim();
            if let Some(text) = code
                .strip_prefix("print(\"")
                .and_then(|r| r.strip_suffix("\")"))
            {
                w.print(text, "doctor_live", 1);
                return Response::Lua(LuaReply {
                    ok: true,
                    values: Vec::new(),
                });
            }
            if code == "return GetSimTime()" {
                Response::Lua(LuaReply {
                    ok: true,
                    values: vec![w.sim_time_ms.to_string()],
                })
            } else if let Some(rest) = code.strip_prefix("return ") {
                Response::Lua(LuaReply {
                    ok: true,
                    values: vec![rest.trim_matches('"').to_owned()],
                })
            } else if code.is_empty() {
                Response::Lua(LuaReply {
                    ok: true,
                    values: Vec::new(),
                })
            } else {
                let message = format!("doctor_live:1: `=' expected near `{code}'");
                w.record(EventBody::LuaError {
                    status: 3,
                    message: message.clone(),
                    chunk: "doctor_live".into(),
                    line: 1,
                    traceback: vec!["[C]: ?".into()],
                });
                if !w.stopped() && w.take_hit(|r| matches!(r.on, BreakOn::LuaError)) {
                    w.stop(
                        StopReason::LuaError {
                            message: message.clone(),
                        },
                        StopPlace::Frame,
                    );
                }
                Response::Lua(LuaReply {
                    ok: false,
                    values: vec![message],
                })
            }
        }
        Request::Events { after, kinds, max } => {
            let kinds = mask::for_version(kinds, version);
            let mut b = w.journal.after(after, kinds, usize::from(max.max(1)));
            b.recording = mask::for_version(mask::ALL_V3, version);
            Response::Events(b)
        }
        Request::Break { rules, hold_s } => {
            let n = rules.len() as u32;
            w.debug.rules = rules;
            w.debug.hold_limit_s = if hold_s == 0 { HOLD_DEFAULT_S } else { hold_s };
            Response::Done(Done {
                count: n,
                note: format!("{n} breakpoint(s)"),
            })
        }
        Request::Control(c) => {
            let note = match c {
                Control::Pause if !w.stopped() => {
                    w.stop(StopReason::Pause, StopPlace::Frame);
                    "paused"
                }
                Control::Pause => "already stopped",
                Control::Continue => {
                    w.debug.stop = None;
                    "goes on"
                }
                Control::Step => {
                    w.debug.stop = None;
                    w.frame += 1;
                    w.sim_time_ms += 50;
                    w.stop(StopReason::Step, StopPlace::Frame);
                    "one frame"
                }
            };
            Response::Done(Done {
                count: 0,
                note: note.into(),
            })
        }
        Request::Debug => Response::Debug(w.debug.clone()),
        Request::SetCamera { camera } => {
            w.debug.camera = camera;
            Response::Done(Done {
                count: u32::from(camera.is_some()),
                note: "free camera".into(),
            })
        }
        Request::Texture { name, data } => match w.textures.iter().find(|t| t.0 == name) {
            None => Response::error(
                ErrorCode::Failed,
                format!("no loaded texture is named {name:#010x}"),
            ),
            Some(&(_, need)) if (data.len() as u64) < need => Response::error(
                ErrorCode::Failed,
                format!("{} bytes, the texture needs {need}", data.len()),
            ),
            Some(&(_, need)) => {
                w.replaced.push((name, data.len()));
                Response::Done(Done {
                    count: 1,
                    note: format!("{need} bytes to the render thread"),
                })
            }
        },
        Request::World { filter, max } => {
            let mut out = World {
                streams: vec![(1, "gf2\\cuba.str".into())],
                ..World::default()
            };
            let all = w
                .objects
                .iter()
                .map(|(o, l)| (o.guid, o.class_name.clone(), Some(o.position), l))
                .chain(w.logic.iter().map(|(g, c, l)| (*g, c.clone(), None, l)));
            for (guid, class_name, position, listens) in all {
                let class = doctor_toolkit_live::name_hash(&class_name);
                if filter.class.is_some_and(|c| c != class)
                    || (!filter.guids.is_empty() && !filter.guids.contains(&guid))
                    || filter.group.is_some_and(|g| g != 1)
                    || filter.positioned.is_some_and(|p| p != position.is_some())
                {
                    continue;
                }
                out.total += 1;
                if (out.objects.len() as u32) < max {
                    if !out.classes.iter().any(|(h, _)| *h == class) {
                        out.classes.push((class, class_name));
                    }
                    out.objects.push(WorldObject {
                        guid,
                        class,
                        group: 1,
                        position,
                        flags: 0x2800_0000,
                        listening: !listens.is_empty(),
                    });
                }
            }
            out.walked = (w.objects.len() + w.logic.len()) as u32;
            Response::World(out)
        }
        Request::TimeOfDay => Response::TimeOfDay(Some(w.time)),
        Request::SetTime(change) => {
            if let Some([h, m, s]) = change.time {
                (w.time.hour, w.time.minute, w.time.second) = (h, m, s);
            }
            if let Some(rate) = change.rate {
                w.time.rate = if rate < 0.0 {
                    w.time.default_rate
                } else {
                    rate
                };
            }
            Response::Done(Done {
                count: 1,
                note: format!(
                    "time of day {:02}:{:02}:{:02}, rate {}",
                    w.time.hour, w.time.minute, w.time.second, w.time.rate
                ),
            })
        }
        Request::Weather => Response::Weather(Weather {
            rain: None,
            time: Some(w.time),
        }),
        Request::StreamLoad { name } => w.stream_request(&name, StreamAction::Load, false),
        Request::StreamUnload { name, force } => {
            w.stream_request(&name, StreamAction::Unload, force)
        }
        Request::StreamQueue => {
            let reply = Response::StreamQueue(StreamQueue {
                busy: false,
                requests: w.stream_requests.clone(),
            });
            w.stream_tick();
            reply
        }
        Request::Money => Response::Money(Some(w.money.clone())),
        Request::SetMoney(change) => {
            let before = w.money.cash;
            let amount = match change {
                MoneyChange::Add(v) => v,
                MoneyChange::Set(v) => v - before,
            };
            w.money.cash = (before + amount).max(0.0);
            w.money.books[5] += amount;
            Response::Done(Done {
                count: 1,
                note: format!("money {} (was {before}, {amount:+})", w.money.cash),
            })
        }
        Request::Family => Response::Family(Family {
            families: vec![FamilyInfo {
                hash: doctor_toolkit_live::addr::PLAYER_FAMILY,
                cash: w.money.cash,
            }],
            crew: w.crew.clone(),
        }),
        Request::Mission => Response::Mission(w.mission.clone()),
        // The fake game opened every redirected file once.
        Request::Files => Response::Files(doctor_toolkit_live::Files {
            hooked: config.hooks_files(),
            logging: config.files_log,
            total: config.redirects.len() as u32,
            redirects: config
                .redirects
                .iter()
                .take(doctor_toolkit_live::files::MAX_LISTED)
                .map(|r| doctor_toolkit_live::FileUse {
                    from: r.from.clone(),
                    to: r.to.clone(),
                    opens: 1,
                    failed: 0,
                })
                .collect(),
            passed: 0,
        }),
        // The fake game opened every patched stream once; the patch is
        // not read.
        Request::Resources => Response::Resources(doctor_toolkit_live::Resources {
            hooked: !config.resources.is_empty(),
            total: config.resources.len() as u32,
            open: 0,
            streams: config
                .resources
                .iter()
                .take(doctor_toolkit_live::resources::MAX_LISTED)
                .map(|r| doctor_toolkit_live::StreamUse {
                    from: r.from.clone(),
                    to: r.to.clone(),
                    opens: 1,
                    ..Default::default()
                })
                .collect(),
        }),
        // No table of resources in the fake game.
        Request::Resource { .. } => Response::Resource(None),
        Request::Variables { hashes } => Response::Variables(
            w.variables
                .iter()
                .filter(|v| hashes.is_empty() || hashes.iter().any(|&h| v.is(h)))
                .copied()
                .collect(),
        ),
        Request::SetVariable { hash, value } => match w.variables.iter_mut().find(|v| v.is(hash)) {
            Some(v) => {
                let before = v.value;
                v.value = value;
                Response::Done(Done {
                    count: 1,
                    note: format!("variable {hash:#010x} = {value} (was {before})"),
                })
            }
            None => Response::error(
                ErrorCode::Failed,
                format!("no variable {hash:#010x} in the loaded streams"),
            ),
        },
        Request::Object { guid } => match w.object_state(&guid) {
            Some(s) => Response::Object(s),
            None => Response::error(
                ErrorCode::Failed,
                format!(
                    "no object {} in the game (its stream is not loaded, or it was never made)",
                    doctor_toolkit_live::guid_hex(&guid)
                ),
            ),
        },
        r @ (Request::ObjectTransform { .. }
        | Request::ObjectEnable { .. }
        | Request::ObjectUndo { .. }) => w.edit(r),
        Request::ObjectEdits => Response::ObjectEdits(w.edits.list().to_vec()),
        Request::ObjectForget { guid } => {
            let n = w.edits.forget(guid.as_ref());
            Response::Done(Done {
                count: n as u32,
                note: format!("forgot {n} edit(s); the game keeps them"),
            })
        }
        Request::Profile {
            reset,
            memory,
            sample_ms,
        } => {
            let p = fake_profile(w, memory, sample_ms);
            if reset {
                w.profile_reset = w.frame;
            }
            Response::Profile(Box::new(p))
        }
        Request::Peek { address, len } => {
            if address < 0x0040_0000 {
                Response::error(
                    ErrorCode::Failed,
                    format!("{address:#010x}+{len} is not readable"),
                )
            } else {
                Response::Peek(
                    (0..len)
                        .map(|i| (address as u8).wrapping_add(i as u8))
                        .collect(),
                )
            }
        }
    }
}

/// Thread ids of the fake game: main loop, render, a loader.
pub const FAKE_THREADS: [u32; 3] = [0x0104, 0x0118, 0x0130];

/// The profile of the fake game (protocol 7): the same for the same frame
/// count and the same last reset.
pub fn fake_profile(w: &FakeWorld, memory: bool, sample_ms: u32) -> doctor_toolkit_live::Profile {
    use doctor_toolkit_live::profile::{
        Clock, CpuTime, MAX_SAMPLE_MS, MemoryMap, Profile, RING, SiteIo, SlowRead, TEXTURES,
        TextureUse, ThreadIo, ThreadSamples,
    };
    let [main, render, loader] = FAKE_THREADS;
    let n = (w.frame.saturating_sub(w.profile_reset) as usize).min(RING);
    // About 30 passes a second, every 50th one slow.
    let pass_us: Vec<u32> = (0..n)
        .map(|i| {
            if i % 50 == 49 {
                120_000
            } else {
                33_000 + (i % 7) as u32 * 100
            }
        })
        .collect();
    let present_interval_us = pass_us.clone();
    let present_us = (0..n).map(|i| 2_000 + (i % 5) as u32 * 100).collect();
    let since = u64::from(w.frame.saturating_sub(w.profile_reset));
    let mut table = vec![0u8; TEXTURES * TextureUse::ENTRY];
    let mut texture =
        |i: usize, state: u8, levels: u8, size: u16, format: u8, flags: u32, pool: u32| {
            let e = &mut table[i * TextureUse::ENTRY..(i + 1) * TextureUse::ENTRY];
            e[0] = state;
            e[1] = levels;
            e[2..4].copy_from_slice(&size.to_le_bytes());
            e[4..6].copy_from_slice(&size.to_le_bytes());
            e[8] = format;
            e[0xC..0x10].copy_from_slice(&flags.to_le_bytes());
            e[0x30..0x34].copy_from_slice(&pool.to_le_bytes());
        };
    texture(0, 2, 1, 1024, 3, 8, 0);
    texture(1, 2, 1, 1024, 0x1A, 8, 0);
    texture(2, 2, 11, 1024, 30, 0, 1);
    texture(3, 2, 9, 256, 33, 2, 1);
    texture(4, 1, 1, 64, 3, 0, 1);
    let sampled = |thread: u32, hot: u32| {
        let total = sample_ms.min(MAX_SAMPLE_MS);
        let top: Vec<(u32, u32)> = [
            (hot, total / 2),
            (0x0060_8D40, total / 4),
            (0x7BC5_1234, total - total / 2 - total / 4),
        ]
        .into_iter()
        .filter(|e| e.1 > 0)
        .collect();
        ThreadSamples {
            thread,
            samples: total,
            failed: 0,
            dropped: 0,
            distinct: top.len() as u32,
            elapsed_ms: total,
            callers: top
                .iter()
                .filter(|e| !super::proto::profile::in_image(e.0))
                .map(|&(_, n)| (0x0040_DFA7, n))
                .collect(),
            top,
        }
    };
    Profile {
        uptime_us: u64::from(w.frame) * 33_333,
        qpc_frequency: 10_000_000,
        reset_at_us: u64::from(w.profile_reset) * 33_333,
        main_thread: main,
        render_thread: render,
        present_hooked: true,
        reads_hooked: true,
        passes: u64::from(w.frame),
        pass_us,
        presents: u64::from(w.frame),
        present_interval_us,
        present_us,
        clock: Clock {
            game_frame: Some(w.frame),
            sim_ms: Some(w.sim_time_ms),
            sim_hz: Some(30.0),
            fixed_step: Some(1),
            vsync: Some(0),
            windowed_limiter: Some(1),
            back_buffer: Some([1280, 720]),
        },
        main_cpu: Some(CpuTime {
            user_ms: w.frame * 20,
            kernel_ms: w.frame,
        }),
        render_cpu: Some(CpuTime {
            user_ms: w.frame * 5,
            kernel_ms: w.frame * 3,
        }),
        process_cpu: Some(CpuTime {
            user_ms: w.frame * 30,
            kernel_ms: w.frame * 5,
        }),
        io: vec![
            ThreadIo {
                thread: main,
                reads: 12 * since as u32,
                bytes: 4_096 * 12 * since,
                total_us: 50 * 12 * since,
                max_us: 900,
                slow: 0,
            },
            ThreadIo {
                thread: loader,
                reads: 4 * since as u32,
                bytes: 262_144 * 4 * since,
                total_us: 6_000 * 4 * since,
                max_us: 25_000,
                slow: 2 * since as u32,
            },
        ],
        io_sites: vec![SiteIo {
            site: doctor_toolkit_live::addr::READ_FILE_CALLS[0],
            reads: 16 * since as u32,
            bytes: (4_096 * 12 + 262_144 * 4) * since,
            total_us: (50 * 12 + 6_000 * 4) * since,
        }],
        slow_reads: (0..since.min(3) as u32)
            .map(|i| SlowRead {
                thread: loader,
                bytes: 262_144,
                us: 25_000 - i * 1_000,
                at_ms: (w.frame - i) * 33,
            })
            .collect(),
        textures: Some(TextureUse::from_table(&table)),
        memory: memory.then_some(MemoryMap {
            committed: 1_400 << 20,
            reserved: 250 << 20,
            free: 2_400 << 20,
            largest_free: 1_100 << 20,
            largest_free_at: 0x9000_0000,
            committed_high: 300 << 20,
            regions: 3_210,
            finished: true,
            walk_ms: 120,
            total_virtual: (4 << 30) - (128 << 10),
            avail_virtual: 2_400 << 20,
        }),
        samples: if sample_ms > 0 {
            vec![sampled(main, 0x0068_B04B), sampled(render, 0x0060_D8B0)]
        } else {
            Vec::new()
        },
    }
}

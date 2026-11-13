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
//! [`FakeWorld::protocol`]).

use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use doctor_toolkit_live::journal::{EventBody, MessageOrigin, OutputSource, Ring, mask};
use doctor_toolkit_live::{
    CameraView, Done, ErrorCode, Guid, Hello, LoopState, LuaReply, ObjectInfo, PROTOCOL_VERSION,
    Pose, Request, Response, Status, StreamInfo, World, WorldObject, read_frame,
};

use super::Config;

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
}

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
        }
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
}

fn answer(request: Request, config: &Config, w: &mut FakeWorld) -> Response {
    if w.wrong_edition {
        return Response::error(ErrorCode::Edition, "not PC 1.0.789.0");
    }
    if request.writes() && !config.writes {
        return Response::error(
            ErrorCode::Denied,
            "writes are switched off in doctor_live.cfg",
        );
    }
    w.frame += 1;
    w.sim_time_ms += 50;
    match request {
        Request::Hello { .. } => unreachable!("handled by serve"),
        Request::Status => Response::Status(Status {
            frame: w.frame,
            state: w.state,
            player: w.player.is_some(),
            sim_time_ms: Some(w.sim_time_ms),
            time_of_day: Some([12, 0, 0]),
        }),
        Request::Player => Response::Player(w.player),
        Request::Camera => Response::Camera(w.camera),
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
                w.record(EventBody::Output {
                    source: OutputSource::Print,
                    text: text.to_owned(),
                    script: "doctor_live".into(),
                    line: 1,
                });
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
                Response::Lua(LuaReply {
                    ok: false,
                    values: vec![message],
                })
            }
        }
        Request::Events { after, kinds, max } => {
            let mut b = w.journal.after(after, kinds, usize::from(max.max(1)));
            b.recording = mask::ALL;
            Response::Events(b)
        }
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

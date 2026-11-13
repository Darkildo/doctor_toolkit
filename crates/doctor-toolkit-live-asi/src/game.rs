//! What the module reads and changes in the game. Runs on the game's main
//! thread inside the frame hook only (between two passes of the main loop),
//! never on the network thread.

use std::arch::asm;

use doctor_toolkit_live::addr::*;
use doctor_toolkit_live::{
    CameraView, Done, ErrorCode, Guid, LoopState, LuaReply, ObjectInfo, Pose, Response, Status,
    StreamInfo,
};

use crate::win::Mem;

/// What every frame records for the replies that do not need to wait for
/// the main thread (status, player, camera).
#[derive(Clone, Default)]
pub struct Snapshot {
    pub frame: u32,
    pub state: u8,
    pub player: Option<Pose>,
    pub camera: Option<CameraView>,
    pub sim_time_ms: Option<u32>,
    pub time_of_day: Option<[u8; 3]>,
}

impl Snapshot {
    pub fn status(&self) -> Status {
        Status {
            frame: self.frame,
            state: LoopState::from_u8(self.state),
            player: self.player.is_some(),
            sim_time_ms: self.sim_time_ms,
            time_of_day: self.time_of_day,
        }
    }
}

fn in_text(p: u32) -> bool {
    (TEXT_START..TEXT_END).contains(&p)
}

/// Current main loop state.
pub fn loop_state(m: &mut Mem) -> LoopState {
    let Some(disp) = m.ptr(LOOP_DISPATCHER) else {
        return LoopState::Unknown;
    };
    let Some(state) = m.ptr(disp + 4) else {
        return LoopState::Unknown;
    };
    match m.u32(state) {
        Some(VTABLE_INIT_LOOP) => LoopState::Init,
        Some(VTABLE_FRONTEND_LOOP) => LoopState::Frontend,
        Some(VTABLE_GAME_LOOP) => LoopState::Game,
        _ => LoopState::Unknown,
    }
}

/// The local player object (`Player`), as `PlayerTeleportRelay` finds it.
pub fn player(m: &mut Mem) -> Option<u32> {
    let list = m.ptr(PLAYER_LIST)?;
    if m.u32(list + 0xC)? == 0 {
        return None;
    }
    let array = m.ptr(list + 4)?;
    let entry = m.ptr(array)?;
    let player = entry.checked_sub(PLAYER_ENTRY_OFFSET)?;
    // The object starts with its vtable, which lies in the image.
    let vtable = m.u32(player)?;
    (0x00D5_5000..0x00E5_0000)
        .contains(&vtable)
        .then_some(player)
}

fn pose_at(m: &mut Mem, matrix: u32) -> Option<Pose> {
    let f: [f32; 16] = m.f32s(matrix)?;
    if f.iter().any(|v| !v.is_finite()) {
        return None;
    }
    Some(Pose {
        right: [f[0], f[1], f[2]],
        up: [f[4], f[5], f[6]],
        forward: [f[8], f[9], f[10]],
        position: [f[12], f[13], f[14]],
    })
}

fn tls_slot0() -> u32 {
    let teb_tls: u32;
    // SAFETY: reads the TLS array pointer of the current thread's TEB.
    unsafe { asm!("mov {}, fs:[0x2c]", out(reg) teb_tls, options(nostack, readonly)) };
    teb_tls
}

/// Vertical field of view in degrees from the camera's frustum planes
/// (`+0x80`: six planes, normal and distance; the two whose normals lean
/// most along the camera's up axis bound it vertically).
fn fov_from_planes(planes: &[f32; 24], up: [f32; 3]) -> Option<f32> {
    let n = |i: usize| [planes[i * 4], planes[i * 4 + 1], planes[i * 4 + 2]];
    let dot = |a: [f32; 3], b: [f32; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    let mut side: Vec<(f32, usize)> = (0..6).map(|i| (dot(n(i), up), i)).collect();
    side.sort_by(|a, b| b.0.total_cmp(&a.0));
    let (top, bottom) = (n(side[0].1), n(side[5].1));
    if side[0].0 < 0.1 || side[5].0 > -0.1 {
        return None;
    }
    let between = dot(top, bottom).clamp(-1.0, 1.0).acos();
    let fov = 180.0 - between.to_degrees();
    (1.0..170.0).contains(&fov).then_some(fov)
}

/// The camera the game renders from: the camera manager's current camera
/// (`0x0040E0A0`; its world matrix at `+0x40`, frustum planes at `+0x80`).
pub fn camera(m: &mut Mem) -> Option<CameraView> {
    let manager = m.ptr(CAMERA_MANAGER)?;
    let cam = if m.u32(manager + CAMERA_FLAG)? == 0 {
        let tls = m.ptr(tls_slot0())?;
        m.ptr(tls + 0x24)?
    } else {
        m.ptr(manager + CAMERA_CURRENT)?
    };
    let f: [f32; 16] = m.f32s(cam + CAMERA_MATRIX)?;
    if !doctor_toolkit_live::is_world_matrix(&f, false) {
        return None;
    }
    let pose = pose_at(m, cam + CAMERA_MATRIX)?;
    let fov = m
        .f32s::<24>(cam + CAMERA_PLANES)
        .and_then(|p| fov_from_planes(&p, pose.up));
    Some(CameraView { pose, fov })
}

/// Hours, minutes, seconds of the time of day manager.
fn time_of_day(m: &mut Mem) -> Option<[u8; 3]> {
    let tod = m.ptr(TIME_OF_DAY)?;
    let h = m.u32(tod + TIME_OF_DAY_HMS)?;
    let min = m.u32(tod + TIME_OF_DAY_HMS + 4)?;
    let s = m.u32(tod + TIME_OF_DAY_HMS + 8)?;
    (h < 24 && min < 60 && s < 60).then_some([h as u8, min as u8, s as u8])
}

pub fn snapshot(frame: u32) -> Snapshot {
    let mut m = Mem::default();
    let state = loop_state(&mut m);
    let player = player(&mut m).and_then(|p| pose_at(&mut m, p + ENTITY_MATRIX));
    let in_game = state == LoopState::Game;
    let camera = if in_game { camera(&mut m) } else { None };
    Snapshot {
        frame,
        state: state as u8,
        player,
        camera,
        sim_time_ms: m.u32(SIM_TIME_MS).filter(|_| in_game),
        time_of_day: if in_game { time_of_day(&mut m) } else { None },
    }
}

/// What [`fix_input`] changed.
pub struct InputFix {
    pub player: u32,
    pub old: u32,
    pub port: u32,
    /// Names of the action maps moved from the old port to the new one.
    pub maps: Vec<String>,
}

/// Gives the player the port of the controller manager when he was made
/// before the manager had one (a start with `-stream`), as if he had been
/// made after it:
///
/// 1. what the constructor of `Player` does at `0x007A2A5A` — the port into
///    `Player+0x2498` and `0x00444D20(port, 1)` (its byte in the input
///    manager);
/// 2. the action maps the player pushed on his old port (`0x0079B5EE`,
///    `basic_aiming_player`) go to the new one: pushed there by name
///    (`0x00464960`), then popped from the old port (`0x00464A10`). The
///    player asks for his actions on his port only.
pub fn fix_input(m: &mut Mem) -> Option<InputFix> {
    let player = player(m)?;
    let manager = m.ptr(CONTROLLER_MANAGER)?;
    let manager_port = m.u8(manager + CONTROLLER_PORT)?;
    let old = m.u32(player + PLAYER_INPUT_PORT)?;
    let port = doctor_toolkit_live::input_port_fix(manager_port, old)?;
    let input = m.ptr(INPUT_MANAGER)?;
    let in_use = input + INPUT_PORT_RECORDS + port * INPUT_PORT_RECORD_SIZE;
    let maps_obj = m.ptr(ACTION_MAPS)?;
    let old_block = maps_obj + old * ACTION_MAPS_PORT_SIZE;
    let count = m.u32(old_block + ACTION_MAPS_COUNT)?;
    if count > 16 {
        return None;
    }
    // The maps of the old port, bottom first, and their names (in the map,
    // alive while the map table holds it).
    let mut names = Vec::new();
    for i in 0..count {
        let map = m.ptr(old_block + ACTION_MAPS_STACK + i * 4)?;
        let name = map + ACTION_MAP_NAME;
        let mut buf = [0u8; 64];
        let len = (0..64u32)
            .take_while(|&j| m.u8(name + j).is_some_and(|b| b != 0))
            .count();
        if len == 0 || len == 64 || !m.bytes(name, &mut buf[..len]) {
            return None;
        }
        names.push((name, String::from_utf8_lossy(&buf[..len]).into_owned()));
    }
    // Both fields readable before anything is written.
    m.u8(in_use)?;
    // SAFETY: fields checked by `edition::check` (the constructor of
    // `Player` writes the same two), read just above; the main thread is
    // between two passes of its loop.
    unsafe {
        std::ptr::write_volatile((player + PLAYER_INPUT_PORT) as usize as *mut u32, port);
        std::ptr::write_volatile(in_use as usize as *mut u8, 1);
    }
    let push: extern "thiscall" fn(u32, u32, u32, u32) -> u8 =
        // SAFETY: `0x00464960`, checked by `edition::check`.
        unsafe { std::mem::transmute(ACTION_MAP_PUSH as usize) };
    let pop: extern "thiscall" fn(u32, u32) -> u32 =
        // SAFETY: `0x00464A10`, checked by `edition::check`.
        unsafe { std::mem::transmute(ACTION_MAP_POP as usize) };
    let mut maps = Vec::new();
    for (name, text) in &names {
        if push(maps_obj, port, *name, 0) & 1 != 0 {
            maps.push(text.clone());
        }
    }
    // Only when every map got onto the new port: else the player keeps them.
    if maps.len() == names.len() {
        for _ in 0..count {
            pop(maps_obj, old);
        }
    }
    Some(InputFix {
        player,
        old,
        port,
        maps,
    })
}

/// Bytes of memory for `Peek`.
pub fn peek(address: u32, len: u16) -> Response {
    let mut out = vec![0u8; len as usize];
    if Mem::default().bytes(address, &mut out) {
        Response::Peek(out)
    } else {
        Response::error(
            ErrorCode::Failed,
            format!("{address:#010x}+{len} is not readable"),
        )
    }
}

/// Calls `bool As(this, class, &out)`, the cast every SimGroup object has
/// (virtual method `+0x10`, e.g. `0x0046D450`).
fn as_class(m: &mut Mem, object: u32, class: u32) -> bool {
    let Some(vtable) = m.u32(object) else {
        return false;
    };
    if !(0x00D5_5000..0x00E5_0000).contains(&vtable) {
        return false;
    }
    let Some(method) = m.u32(vtable + OBJECT_AS_METHOD).filter(|&p| in_text(p)) else {
        return false;
    };
    let f: extern "thiscall" fn(u32, u32, *mut u32) -> u8 =
        // SAFETY: a method of the object's vtable in the game's code, with
        // the signature of `0x0046D450` (it only compares and stores).
        unsafe { std::mem::transmute(method as usize) };
    let mut out = 0u32;
    f(object, class, &mut out) & 1 != 0 && out != 0
}

/// Registered classes: hash, size, name.
fn class_registry(m: &mut Mem) -> Vec<(u32, u32, String)> {
    let (Some(array), Some(count)) = (m.ptr(CLASS_REGISTRY), m.u32(CLASS_REGISTRY + 4)) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for i in 0..count.min(4096) {
        let e = array + i * 16;
        let (Some(hash), Some(name), Some(size)) = (m.u32(e), m.ptr(e + 8), m.u32(e + 12)) else {
            continue;
        };
        let mut buf = [0u8; 64];
        let len = (0..64u32)
            .take_while(|&j| m.u8(name + j).is_some_and(|b| b != 0))
            .count();
        let text = if m.bytes(name, &mut buf[..len]) {
            String::from_utf8_lossy(&buf[..len]).into_owned()
        } else {
            String::new()
        };
        out.push((hash, size, text));
    }
    out
}

/// The most derived registered class of an object: of the classes it can
/// be cast to, the largest.
fn class_of(m: &mut Mem, object: u32, registry: &[(u32, u32, String)]) -> (u32, String) {
    registry
        .iter()
        .filter(|(hash, _, _)| as_class(m, object, *hash))
        .max_by_key(|(_, size, _)| *size)
        .map(|(h, _, n)| (*h, n.clone()))
        .unwrap_or_default()
}

/// SimGroup objects near `center`, by walking the class table of the
/// SimManager (`0x004467C0`). An object has a position when a world matrix
/// is at `+0xD0` (`Entity` and the classes made from it); the walk calls no
/// game code and stops after a time budget.
pub fn objects(center: Option<[f32; 3]>, radius: f32, class: Option<u32>, max: u16) -> Response {
    let started = std::time::Instant::now();
    let budget = std::time::Duration::from_millis(400);
    let mut m = Mem::mapped();
    let center = match center {
        Some(c) => c,
        None => match player(&mut m).and_then(|p| pose_at(&mut m, p + ENTITY_MATRIX)) {
            Some(p) => p.position,
            None => return Response::error(ErrorCode::NotReady, "no player and no centre"),
        },
    };
    let Some(sim) = m.ptr(SIM_MANAGER) else {
        return Response::error(ErrorCode::NotReady, "no SimManager");
    };
    let (Some(buckets), Some(count)) = (
        m.ptr(sim + SIM_CLASS_BUCKETS),
        m.u32(sim + SIM_CLASS_BUCKET_COUNT),
    ) else {
        return Response::error(ErrorCode::NotReady, "no class table");
    };
    let mut found = Vec::new();
    let mut seen = 0u32;
    let mut classes = 0u32;
    let mut cut = false;
    'buckets: for i in 0..count.min(1 << 16) {
        let mut record = m.u32(buckets + i * 4).unwrap_or(0);
        let mut records = 0;
        while record >= 0x1_0000 && records < 1024 {
            records += 1;
            classes += 1;
            {
                let mut object = m.u32(record + 0x10).unwrap_or(0);
                let mut n = 0;
                while object >= 0x1_0000 && n < 100_000 {
                    n += 1;
                    seen += 1;
                    if seen.is_multiple_of(1024) && started.elapsed() > budget {
                        cut = true;
                        break 'buckets;
                    }
                    if let Some(f) = m.f32s::<16>(object + ENTITY_MATRIX)
                        && doctor_toolkit_live::is_world_matrix(&f, true)
                    {
                        let pos = [f[12], f[13], f[14]];
                        let d = ((pos[0] - center[0]).powi(2)
                            + (pos[1] - center[1]).powi(2)
                            + (pos[2] - center[2]).powi(2))
                        .sqrt();
                        let mut guid: Guid = [0; 16];
                        if d <= radius && m.bytes(object + OBJECT_GUID, &mut guid) {
                            found.push((
                                object,
                                ObjectInfo {
                                    guid,
                                    class: 0,
                                    class_name: String::new(),
                                    position: pos,
                                    distance: d,
                                },
                            ));
                        }
                    }
                    object = m.u32(object + 8).unwrap_or(0);
                }
            }
            record = m.u32(record + 0x18).unwrap_or(0);
        }
    }
    found.sort_by(|a, b| a.1.distance.total_cmp(&b.1.distance));
    // The class of the nearest ones only: up to a few hundred casts each.
    let registry = class_registry(&mut m);
    let mut list = Vec::new();
    for (object, mut info) in found {
        if list.len() >= max as usize {
            break;
        }
        // A class filter keeps that class and the classes made from it.
        if class.is_some_and(|c| !as_class(&mut m, object, c)) {
            continue;
        }
        (info.class, info.class_name) = class_of(&mut m, object, &registry);
        {
            list.push(info);
        }
    }
    let found = list;
    crate::log(&format!(
        "objects: {classes} classes, {seen} objects walked in {} ms{}, {} returned",
        started.elapsed().as_millis(),
        if cut { " (time budget reached)" } else { "" },
        found.len()
    ));
    Response::Objects(found)
}

/// Loaded streams (`0x00405F40`).
pub fn streams() -> Response {
    let mut m = Mem::default();
    let Some(manager) = m.ptr(STREAM_MANAGER) else {
        return Response::error(ErrorCode::NotReady, "no StreamManager");
    };
    let mut out = Vec::new();
    let mut node = m.u32(manager + STREAM_LIST).unwrap_or(0);
    let mut n = 0;
    while node >= 0x1_0000 && n < 1024 {
        n += 1;
        let name = stream_name(&mut m, node);
        out.push(StreamInfo { name });
        node = m.u32(node + 4).unwrap_or(0);
    }
    Response::Streams(out)
}

/// Name of a loaded stream (`gf2\\cuba.str`): a record of the list at
/// `StreamManager+0x6C` holds a pointer to it at `+8` and its length at
/// `+0xC`.
fn stream_name(m: &mut Mem, node: u32) -> String {
    let len = m.u32(node + 0xC).unwrap_or(0).min(260) as usize;
    let mut buf = vec![0u8; len];
    match m.ptr(node + 8) {
        Some(p) if len > 0 && m.bytes(p, &mut buf) => {
            let end = buf.iter().position(|&b| b == 0).unwrap_or(len);
            String::from_utf8_lossy(&buf[..end]).into_owned()
        }
        _ => format!("? (record {node:#010x})"),
    }
}

#[repr(C, align(16))]
struct Matrix([f32; 16]);

/// Moves the player the way `PlayerTeleportRelay` does (`0x007DD020`).
pub fn teleport(position: [f32; 3], yaw: Option<f32>) -> Response {
    let mut m = Mem::default();
    if loop_state(&mut m) != LoopState::Game {
        return Response::error(ErrorCode::NotReady, "not in a level");
    }
    if m.ptr(FRAME_COMMANDS).is_none() {
        return Response::error(ErrorCode::NotReady, "no frame in progress");
    }
    let Some(player) = player(&mut m) else {
        return Response::error(ErrorCode::NotReady, "no player");
    };
    let Some(state) = m.u32(player + PLAYER_STATE) else {
        return Response::error(ErrorCode::NotReady, "player state unreadable");
    };
    if state >> 7 & 1 != 0 {
        return Response::error(
            ErrorCode::NotReady,
            "the game does not move the player now (cutscene, vehicle or loading)",
        );
    }
    let Some(now) = pose_at(&mut m, player + ENTITY_MATRIX) else {
        return Response::error(ErrorCode::NotReady, "player matrix unreadable");
    };
    if position.iter().any(|v| !v.is_finite() || v.abs() > 1.0e5) {
        return Response::error(ErrorCode::BadRequest, "position out of range");
    }
    let (right, forward) = match yaw {
        Some(a) => ([a.cos(), 0.0, -a.sin()], [a.sin(), 0.0, a.cos()]),
        None => (now.right, now.forward),
    };
    let up = if yaw.is_some() {
        [0.0, 1.0, 0.0]
    } else {
        now.up
    };
    let matrix = Matrix([
        right[0],
        right[1],
        right[2],
        0.0,
        up[0],
        up[1],
        up[2],
        0.0,
        forward[0],
        forward[1],
        forward[2],
        0.0,
        position[0],
        position[1],
        position[2],
        1.0,
    ]);
    // The message to send when done (none) and the world partitions to load
    // (none: the game streams zones by the player's position).
    let done = [0u32; 2];
    let partitions = [0u32; 5];
    let Some(vtable) = m.u32(player) else {
        return Response::error(ErrorCode::Failed, "player vtable unreadable");
    };
    let Some(prepare) = m
        .u32(vtable + PLAYER_PREPARE_METHOD)
        .filter(|&p| in_text(p))
    else {
        return Response::error(ErrorCode::Failed, "player method not in the game's code");
    };
    // SAFETY: the calls and arguments of `0x007DD062` and `0x007DD117`, on
    // the main thread between frames, with the player checked above.
    unsafe {
        let prepare: extern "thiscall" fn(u32, u32, u32, u32) =
            std::mem::transmute(prepare as usize);
        prepare(player, PLAYER_PREPARE_ARG, 1, 0);
        let teleport: extern "thiscall" fn(u32, *const Matrix, u32, *const u32, *const u32) =
            std::mem::transmute(PLAYER_TELEPORT as usize);
        teleport(player, &matrix, 0, done.as_ptr(), partitions.as_ptr());
    }
    Response::Done(Done {
        count: 1,
        note: format!(
            "teleport from ({:.2}, {:.2}, {:.2}) to ({:.2}, {:.2}, {:.2})",
            now.position[0],
            now.position[1],
            now.position[2],
            position[0],
            position[1],
            position[2]
        ),
    })
}

#[repr(C)]
struct Message {
    hash: u32,
    data: u32,
    sent: u8,
}

/// The table entry of a message: `[0]` next, `[1]` listener blocks, `[4]`
/// hash (`0x00408A00`).
fn message_entry(m: &mut Mem, hash: u32) -> Option<u32> {
    let table = m.ptr(MESSAGE_TABLE)?;
    let mut entry = m.u32(table + (hash & 0xFFF) * 4)?;
    let mut n = 0;
    while entry >= 0x1_0000 && n < 100_000 {
        n += 1;
        if m.u32(entry + 0x10)? == hash {
            return Some(entry);
        }
        entry = m.u32(entry)?;
    }
    None
}

/// Listener objects of a message entry, as the bus walks them; with
/// `enabled_only` those the bus would call.
fn listeners(m: &mut Mem, entry: u32, enabled_only: bool) -> Vec<u32> {
    let mut out = Vec::new();
    let mut block = m.u32(entry + 4).unwrap_or(0);
    let mut blocks = 0;
    while block >= 0x1_0000 && blocks < 10_000 {
        blocks += 1;
        let mut node = m.u32(block + 8).unwrap_or(0);
        let mut n = 0;
        while node >= 0x1_0000 && n < 100_000 {
            n += 1;
            let flags = m.u32(node + 0x14).map(|f| f as u16).unwrap_or(0x8000);
            if let Some(listener) = m.ptr(node + 0xC)
                && flags & 0x8000 == 0
            {
                let on = m.u8(listener + 4).unwrap_or(0) & 1 != 0;
                if on || !enabled_only {
                    out.push(listener);
                }
            }
            node = m.u32(node + 4).unwrap_or(0);
        }
        block = m.u32(block).unwrap_or(0);
    }
    out
}

/// Sends a message: to all listeners through the game's bus
/// (`0x00408A00`, as Lua `SendMessage` does), or to the listener of one
/// object.
pub fn send(hash: u32, target: Option<Guid>) -> Response {
    let mut m = Mem::default();
    if loop_state(&mut m) != LoopState::Game {
        return Response::error(ErrorCode::NotReady, "not in a level");
    }
    let entry = message_entry(&mut m, hash);
    let mut msg = Message {
        hash,
        data: 0,
        sent: 0,
    };
    match target {
        None => {
            let n = entry.map(|e| listeners(&mut m, e, true).len()).unwrap_or(0);
            // SAFETY: the bus function with a message like `0x004C82C0`
            // builds, on the main thread between frames.
            let delivered = unsafe {
                let f: extern "C" fn(*mut Message, u32) -> u8 =
                    std::mem::transmute(MESSAGE_SEND as usize);
                f(&mut msg, 0) & 1 != 0
            };
            Response::Done(Done {
                count: n as u32,
                note: if delivered {
                    format!("{n} listener(s)")
                } else {
                    "nobody listens to this message".to_owned()
                },
            })
        }
        Some(guid) => {
            let Some(entry) = entry else {
                return Response::Done(Done {
                    count: 0,
                    note: "nobody listens to this message".to_owned(),
                });
            };
            let mut hits = 0;
            for listener in listeners(&mut m, entry, true) {
                let object = listener - OBJECT_LISTENER;
                let mut g: Guid = [0; 16];
                if !m.bytes(object + OBJECT_GUID, &mut g) || g != guid {
                    continue;
                }
                let Some(vtable) = m.u32(listener) else {
                    continue;
                };
                let Some(method) = m.u32(vtable + 4).filter(|&p| in_text(p)) else {
                    continue;
                };
                msg.sent = 1;
                // SAFETY: what the bus calls for each listener
                // (`0x00408A00`), only for the listener of this object.
                unsafe {
                    let f: extern "thiscall" fn(u32, *mut Message) =
                        std::mem::transmute(method as usize);
                    f(listener, &mut msg);
                }
                hits += 1;
            }
            Response::Done(Done {
                count: hits,
                note: if hits == 0 {
                    "the object does not listen to this message".to_owned()
                } else {
                    "delivered".to_owned()
                },
            })
        }
    }
}

const LUA_CHUNK_NAME: &[u8] = b"=doctor_live\0";

/// One value of the Lua stack as text (`TValue` of Lua 5.0 with `float`
/// numbers: type, value).
fn lua_value(m: &mut Mem, at: u32) -> String {
    let (Some(tt), Some(v)) = (m.u32(at), m.u32(at + 4)) else {
        return "?".to_owned();
    };
    match tt {
        0 => "nil".to_owned(),
        1 => (v != 0).to_string(),
        3 => {
            let f = f32::from_bits(v);
            if f.fract() == 0.0 && f.abs() < 1.0e9 {
                format!("{}", f as i64)
            } else {
                format!("{f}")
            }
        }
        4 => {
            let len = m.u32(v + 0xC).unwrap_or(0).min(4096) as usize;
            let mut buf = vec![0u8; len];
            if m.bytes(v + 0x10, &mut buf) {
                String::from_utf8_lossy(&buf).into_owned()
            } else {
                "?".to_owned()
            }
        }
        2 => format!("lightuserdata: {v:#010x}"),
        5 => format!("table: {v:#010x}"),
        6 => format!("function: {v:#010x}"),
        7 => format!("userdata: {v:#010x}"),
        8 => format!("thread: {v:#010x}"),
        _ => format!("type {tt}"),
    }
}

/// Runs Lua code in the game's state: `luaL_loadbuffer` and a protected
/// call like `lua_dobuffer`, with the results read off the stack.
pub fn lua(code: &str) -> Response {
    let mut m = Mem::default();
    if loop_state(&mut m) != LoopState::Game {
        return Response::error(ErrorCode::NotReady, "not in a level (no Lua state)");
    }
    let Some(l) = m
        .ptr(LUA_MANAGER)
        .and_then(|mgr| m.ptr(mgr + LUA_MANAGER_STATE))
    else {
        return Response::error(ErrorCode::NotReady, "no Lua state");
    };
    let (Some(top), Some(stack)) = (m.u32(l + 8), m.u32(l + 0x1C)) else {
        return Response::error(ErrorCode::NotReady, "Lua state unreadable");
    };
    let bytes = code.as_bytes();
    let mut status: u32;
    // SAFETY: `0x00629280` with its register arguments (buffer in ECX,
    // name in EAX; L and size on the stack), on the main thread between
    // frames, when no Lua code runs.
    unsafe {
        asm!(
            "push edx",
            "push edi",
            "mov edx, {load}",
            "call edx",
            "add esp, 8",
            load = const LUA_LOAD_BUFFER,
            inout("edx") bytes.len() => _,
            in("edi") l,
            inout("ecx") bytes.as_ptr() => _,
            inout("eax") LUA_CHUNK_NAME.as_ptr() => status,
            clobber_abi("C"),
        );
    }
    if status == 0 {
        // The chunk is at top; call it with LUA_MULTRET.
        let func = top;
        let call = [func, u32::MAX];
        let old_top = func - stack;
        // SAFETY: `luaD_pcall(f_call, &{func, -1}, old_top)` with L in ESI
        // and no error function, as `0x006293C0` calls it.
        unsafe {
            asm!(
                "push esi",
                "mov esi, edi",
                "push eax",
                "push ecx",
                "push {fcall}",
                "xor edx, edx",
                "mov eax, {pcall}",
                "call eax",
                "add esp, 12",
                "pop esi",
                fcall = const LUA_F_CALL,
                pcall = const LUA_PCALL,
                in("edi") l,
                inout("eax") old_top => status,
                inout("ecx") call.as_ptr() => _,
                out("edx") _,
                clobber_abi("C"),
            );
        }
    }
    let end = m.u32(l + 8).unwrap_or(top);
    let mut values = Vec::new();
    let mut at = top;
    while at + 8 <= end && values.len() < 32 {
        values.push(lua_value(&mut m, at));
        at += 8;
    }
    // Drop what the call left (lua_settop to the old top).
    if end > top {
        // SAFETY: lowering `L->top` is what `lua_settop` does.
        unsafe { std::ptr::write_volatile((l + 8) as usize as *mut u32, top) };
    }
    if status != 0 && values.is_empty() {
        values.push(format!("Lua error {status}"));
    }
    Response::Lua(LuaReply {
        ok: status == 0,
        values,
    })
}

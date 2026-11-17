//! The protocol between the editor and the module in the game.
//!
//! A TCP connection to `127.0.0.1:PORT`; the client sends requests, the
//! module answers each with one reply carrying the same id, in order. Every
//! frame starts with a 16-byte header:
//!
//! | Bytes | Field |
//! |---|---|
//! | 0–3 | `DTLV` |
//! | 4–5 | protocol version ([`PROTOCOL_VERSION`]) |
//! | 6–7 | kind: a request, or the request's kind with bit 15 for its reply, `0xFFFF` for an error |
//! | 8–11 | request id, echoed in the reply |
//! | 12–15 | payload length (at most [`MAX_PAYLOAD`]) |
//!
//! Numbers are little-endian, strings are a u16 length and UTF-8 bytes,
//! lists a u32 count. The first request of a connection is [`Request::Hello`]
//! with the token of the module's configuration.
//!
//! **Versions.** Version 2 (stage 49) adds [`Request::Events`] (the game's
//! journal) and [`Request::World`] (every object). A module answers frames
//! of every version from [`OLDEST_PROTOCOL_VERSION`] to
//! [`PROTOCOL_VERSION`] in the version of the request, so clients of
//! version 1 keep working; a request newer than its frame's version gets
//! [`ErrorCode::Unsupported`]. A frame of another version gets
//! [`ErrorCode::Version`] and the connection is closed — a client of
//! version 2 then connects again with version 1 and does without the new
//! requests (a module of stage 34).
//!
//! Version 3 (stage 35) adds live debugging ([`crate::debug`]):
//! breakpoints ([`Request::Break`]), pause and step ([`Request::Control`]),
//! their state ([`Request::Debug`]), the free camera
//! ([`Request::SetCamera`]), the replacement of a texture
//! ([`Request::Texture`]) and counts of the messages the journal does not
//! record one by one ([`crate::journal::mask::MESSAGE_COUNTS`]).
//!
//! Version 4 (stage 48) adds the world and the state of the game
//! ([`crate::state`]): the time of day and its rate ([`Request::TimeOfDay`],
//! [`Request::SetTime`]), the weather ([`Request::Weather`]), streams loaded
//! and unloaded one at a time ([`Request::StreamLoad`],
//! [`Request::StreamUnload`], [`Request::StreamQueue`]), money
//! ([`Request::Money`], [`Request::SetMoney`]), the family
//! ([`Request::Family`]), missions ([`Request::Mission`]) and the variables
//! of the story ([`Request::Variables`], [`Request::SetVariable`]).
//!
//! Version 5 (stage 46) adds [`Request::Files`]: the files the game opens
//! from the toolkit's folder instead of its own ([`crate::files`]) and how
//! often it opened each.
//!
//! Version 6 (stage 47) edits the world while the game runs
//! ([`crate::edit`]): the state of one object ([`Request::Object`]), its
//! matrix ([`Request::ObjectTransform`]), hiding and showing it or its
//! logic ([`Request::ObjectEnable`]), undo ([`Request::ObjectUndo`]) and
//! the module's journal of edits ([`Request::ObjectEdits`],
//! [`Request::ObjectForget`]).
//!
//! Version 7 (stage 58) adds [`Request::Profile`]: the pace of the main
//! loop and of the presents, the reads of files, the CPU time of the
//! threads, the textures, the address space and the clock of the game,
//! and a sampler of where its threads stand ([`crate::profile`]).
//!
//! Version 8 (stage 78) adds no request: a client that asks for
//! [`crate::journal::mask::MESSAGE_SENDER`] gets message events with their
//! origin ([`crate::journal::MessageFrom`]: the place of the call, the
//! object whose handler sent it, the object in the data) and the deliveries
//! to one listener, which pass the broadcast of the bus by.
//!
//! Version 9 (stage 100) adds [`Request::Resources`] — the streams the
//! module composes in memory from the game's file and a resource patch
//! ([`crate::resources`]) and their use — and [`Request::Resource`]: the
//! record of a loaded resource in the game's table of resources, by its
//! GUID (where its data lie in memory, for comparing them).
//!
//! Writes into the game ([`Request::Teleport`], [`Request::Send`],
//! [`Request::Lua`]) are separate requests: the editor asks before it sends
//! them, and the module refuses them when its configuration says
//! `writes=0`.

use std::fmt;
use std::io::{Read, Write};

pub use crate::debug::{BreakOn, BreakRule, Control, DebugState, Stop, StopPlace, StopReason};
pub use crate::edit::{EditJournal, EditKind, EditValue, Matrix, ObjectEdit, ObjectState};
pub use crate::files::{FileUse, Files};
pub use crate::journal::{Event, EventBatch, EventBody, MessageFrom, MessageOrigin, OutputSource};
pub use crate::profile::{
    Clock, CpuTime, FormatUse, MemoryMap, Profile, SiteIo, SlowRead, Stats, TextureUse, ThreadIo,
    ThreadSamples,
};
pub use crate::resources::{Loaded, Resources, StreamUse};
pub use crate::state::{
    Checkpoint, CrewSlot, Family, FamilyInfo, Mission, Money, MoneyChange, Rain, StreamAction,
    StreamQueue, StreamRequest, TimeChange, TimeOfDay, Variable, Weather,
};
use crate::wire::{Reader, Writer};
pub use crate::world::{World, WorldFilter, WorldObject};

/// Magic of every frame.
pub const MAGIC: [u8; 4] = *b"DTLV";
/// Version of this protocol: what a client sends first.
pub const PROTOCOL_VERSION: u16 = 9;
/// Oldest version a module still answers.
pub const OLDEST_PROTOCOL_VERSION: u16 = 1;

/// Whether this side speaks a frame of `version`.
pub fn supported(version: u16) -> bool {
    (OLDEST_PROTOCOL_VERSION..=PROTOCOL_VERSION).contains(&version)
}
/// Size of a frame header.
pub const HEADER_LEN: usize = 16;
/// Largest payload either side sends or accepts (32 MB since protocol 3:
/// a texture with its levels; the rest stays far below 1 MB).
pub const MAX_PAYLOAD: usize = 32 << 20;
/// Kind bit of a reply.
pub const REPLY: u16 = 0x8000;
/// Kind of an error reply.
pub const ERROR_KIND: u16 = 0xFFFF;

/// A 128-bit GUID of a SimGroup object, in memory order.
pub type Guid = [u8; 16];

/// What went wrong while reading a frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodeError {
    Truncated,
    BadMagic,
    TooLong(u32),
    UnknownKind(u16),
    BadUtf8,
    BadValue(&'static str),
    Trailing(usize),
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Truncated => write!(f, "frame is cut short"),
            Self::BadMagic => write!(f, "not a frame of the live link (no DTLV)"),
            Self::TooLong(n) => write!(f, "payload of {n} bytes is too long"),
            Self::UnknownKind(k) => write!(f, "unknown kind {k:#06x}"),
            Self::BadUtf8 => write!(f, "string is not UTF-8"),
            Self::BadValue(what) => write!(f, "bad value of {what}"),
            Self::Trailing(n) => write!(f, "{n} bytes left after the payload"),
        }
    }
}

impl std::error::Error for DecodeError {}

/// A frame header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    pub version: u16,
    pub kind: u16,
    pub id: u32,
    pub len: u32,
}

impl Header {
    pub fn to_bytes(self) -> [u8; HEADER_LEN] {
        let mut out = [0u8; HEADER_LEN];
        out[0..4].copy_from_slice(&MAGIC);
        out[4..6].copy_from_slice(&self.version.to_le_bytes());
        out[6..8].copy_from_slice(&self.kind.to_le_bytes());
        out[8..12].copy_from_slice(&self.id.to_le_bytes());
        out[12..16].copy_from_slice(&self.len.to_le_bytes());
        out
    }

    /// Reads a header. A header of another version is returned as it is
    /// (the receiver answers [`ErrorCode::Version`]); a wrong magic or a
    /// payload over [`MAX_PAYLOAD`] is an error.
    pub fn parse(bytes: &[u8; HEADER_LEN]) -> Result<Self, DecodeError> {
        if bytes[0..4] != MAGIC {
            return Err(DecodeError::BadMagic);
        }
        let u16_at = |i: usize| u16::from_le_bytes([bytes[i], bytes[i + 1]]);
        let u32_at =
            |i: usize| u32::from_le_bytes([bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]]);
        let header = Self {
            version: u16_at(4),
            kind: u16_at(6),
            id: u32_at(8),
            len: u32_at(12),
        };
        if header.len as usize > MAX_PAYLOAD {
            return Err(DecodeError::TooLong(header.len));
        }
        Ok(header)
    }
}

/// One frame of [`PROTOCOL_VERSION`]: header and payload.
pub fn frame(kind: u16, id: u32, payload: &[u8]) -> Vec<u8> {
    frame_of(PROTOCOL_VERSION, kind, id, payload)
}

/// One frame of `version`.
pub fn frame_of(version: u16, kind: u16, id: u32, payload: &[u8]) -> Vec<u8> {
    let header = Header {
        version,
        kind,
        id,
        len: payload.len() as u32,
    };
    let mut out = Vec::with_capacity(HEADER_LEN + payload.len());
    out.extend_from_slice(&header.to_bytes());
    out.extend_from_slice(payload);
    out
}

/// Reads one frame. I/O errors and broken headers come back as
/// [`std::io::Error`] (kind `InvalidData` for the latter).
pub fn read_frame(r: &mut impl Read) -> std::io::Result<(Header, Vec<u8>)> {
    let mut head = [0u8; HEADER_LEN];
    r.read_exact(&mut head)?;
    let header = Header::parse(&head)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    let mut payload = vec![0u8; header.len as usize];
    r.read_exact(&mut payload)?;
    Ok((header, payload))
}

/// Writes one frame.
pub fn write_frame(w: &mut impl Write, kind: u16, id: u32, payload: &[u8]) -> std::io::Result<()> {
    w.write_all(&frame(kind, id, payload))?;
    w.flush()
}

/// A message of the SimGroup bus: by name (hashed like the engine does,
/// [`crate::name_hash`]) or by its hash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MessageId {
    Name(String),
    Hash(u32),
}

impl MessageId {
    pub fn hash(&self) -> u32 {
        match self {
            Self::Name(n) => crate::name_hash(n),
            Self::Hash(h) => *h,
        }
    }

    /// `0x1234ABCD` is a hash, anything else a name.
    pub fn parse(text: &str) -> Self {
        let t = text.trim();
        match t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
            Some(hex) if hex.len() <= 8 && !hex.is_empty() => u32::from_str_radix(hex, 16)
                .map(Self::Hash)
                .unwrap_or_else(|_| Self::Name(t.to_owned())),
            _ => Self::Name(t.to_owned()),
        }
    }
}

impl fmt::Display for MessageId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Name(n) => write!(f, "{n}"),
            Self::Hash(h) => write!(f, "{h:#010x}"),
        }
    }
}

/// A request to the module.
#[derive(Debug, Clone, PartialEq)]
pub enum Request {
    /// First request of a connection: the token of the configuration and
    /// who is asking.
    Hello { token: String, client: String },
    /// Main loop state, frame counter, game time.
    Status,
    /// The local player: position and orientation.
    Player,
    /// The camera the game renders with.
    Camera,
    /// Streams the game has loaded.
    Streams,
    /// SimGroup objects with a position within `radius` metres of `center`
    /// (the player when `None`), nearest first, at most `max`; `class`
    /// keeps one class (hash of its name).
    Objects {
        center: Option<[f32; 3]>,
        radius: f32,
        class: Option<u32>,
        max: u16,
    },
    /// Writes: moves the player to `position` the way `PlayerTeleportRelay`
    /// does; `yaw` (radians, 0 = +Z) turns him, else he keeps facing as he
    /// was.
    Teleport {
        position: [f32; 3],
        yaw: Option<f32>,
    },
    /// Writes: sends a message on the SimGroup bus — to every listener like
    /// the game does, or only to the object `target`.
    Send {
        message: MessageId,
        target: Option<Guid>,
    },
    /// Writes: runs Lua 5.0 code in the game's state; its return values come
    /// back as text.
    Lua { code: String },
    /// Reads `len` bytes of the game's memory at `address` (diagnostics;
    /// unreadable memory is an error, never a crash).
    Peek { address: u32, len: u16 },
    /// Version 2: events of the game's journal numbered after `after` whose
    /// kind is in `kinds` ([`crate::journal::mask`]), at most `max`. Asking
    /// for messages switches their recording on for a while (the module
    /// does not watch the bus for nobody).
    Events { after: u64, kinds: u8, max: u16 },
    /// Version 2: every SimGroup object of the game that passes `filter`,
    /// at most `max` ([`crate::world::MAX_WORLD_OBJECTS`]).
    World { filter: WorldFilter, max: u32 },
    /// Version 3: the breakpoints (all of them: an empty list clears) and
    /// how long a stop may last before the game goes on by itself
    /// (seconds, at most [`crate::debug::HOLD_MAX_S`]). Not a write: the
    /// game's data stay as they are.
    Break { rules: Vec<BreakRule>, hold_s: u16 },
    /// Version 3: pause, go on, one frame.
    Control(Control),
    /// Version 3: breakpoints, whether and where the game stands.
    Debug,
    /// Version 3, writes: render with this camera instead of the game's,
    /// every frame, until `None`.
    SetCamera { camera: Option<CameraView> },
    /// Version 3, writes: new pixels for the loaded texture whose name hash
    /// (the last id of its `tg4h`/`tg4d` resources) is `name`: the whole
    /// `tg4d` payload, every level, of the same size and format.
    Texture { name: u32, data: Vec<u8> },
    /// Version 4: the game's clock.
    TimeOfDay,
    /// Version 4, writes: sets the time of day and its rate.
    SetTime(TimeChange),
    /// Version 4: rain and what decides the sky.
    Weather,
    /// Version 4, writes: asks the streamer for a stream (`gf2\\cuba\\…`,
    /// [`crate::state::stream_name`]) the way `LoadStream` does
    /// (`0x00403A50`). The module refuses while an earlier request of the
    /// link is not done or the game is loading a stream: two requests at
    /// once crash the game.
    StreamLoad { name: String },
    /// Version 4, writes: unloads a stream the way `UnloadStream` does
    /// (`0x00404C40`); only one the link loaded unless `force`.
    StreamUnload { name: String, force: bool },
    /// Version 4: the stream requests of the link and whether they are done.
    StreamQueue,
    /// Version 4: the player's money.
    Money,
    /// Version 4, writes: adds to the player's money or sets it, through
    /// the call the game's rewards use (`0x008C09A0`).
    SetMoney(MoneyChange),
    /// Version 4: the families and the player's crew.
    Family,
    /// Version 4: the current checkpoint and those of the loaded streams.
    Mission,
    /// Version 4: the variables of the story whose name or record hash is
    /// in `hashes` (all of them when empty).
    Variables { hashes: Vec<u32> },
    /// Version 4, writes: sets a variable (by the hash of its name or of
    /// its record) and wakes those who watch it (`0x00442240`), as
    /// `VariableOperator` does.
    SetVariable { hash: u32, value: i32 },
    /// Version 5: the files the game opens from elsewhere and their use
    /// ([`crate::files`]).
    Files,
    /// Version 6: the state of one SimGroup object ([`crate::edit`]).
    Object { guid: Guid },
    /// Version 6, writes: gives an object a new matrix (rows right, up,
    /// forward, translation, with its scale) through the game's own
    /// `Entity` method; the module takes the object out of the world and
    /// puts it back, so its collision follows.
    ObjectTransform { guid: Guid, matrix: Matrix },
    /// Version 6, writes: hides or shows an object (`logic` false: its model
    /// and collision, through the game's count of deactivations) or stops
    /// and starts its logic (`logic` true: whether its listener takes
    /// messages). Showing is only for what the link hid.
    ObjectEnable {
        guid: Guid,
        enabled: bool,
        logic: bool,
    },
    /// Version 6, writes: undoes the latest edit of the link (of `guid`
    /// when given) and takes it out of the journal.
    ObjectUndo { guid: Option<Guid> },
    /// Version 6: the edits of the link not undone, oldest first.
    ObjectEdits,
    /// Version 6: drops the edits of `guid` (all when `None`) from the
    /// journal without undoing them — after they went into a workspace.
    ObjectForget { guid: Option<Guid> },
    /// Version 7: the profile of the game measured from inside
    /// ([`crate::profile`]). `reset` empties the rings and the counters of
    /// reads after the answer is made; `memory` walks the address space
    /// (up to 2 s); `sample_ms` (at most
    /// [`crate::profile::MAX_SAMPLE_MS`]) samples where the main and the
    /// render thread stand first. Reads only.
    Profile {
        reset: bool,
        memory: bool,
        sample_ms: u32,
    },
    /// Version 9: the streams the module composes from the game's file and
    /// a resource patch, and their use ([`crate::resources`]).
    Resources,
    /// Version 9: the record of the loaded resource with these four ids in
    /// the game's table of resources (`*0x012053AC`). Reads only.
    Resource { ids: [u32; 4] },
}

/// Kinds of requests.
pub mod kind {
    pub const HELLO: u16 = 1;
    pub const STATUS: u16 = 2;
    pub const PLAYER: u16 = 3;
    pub const CAMERA: u16 = 4;
    pub const STREAMS: u16 = 5;
    pub const OBJECTS: u16 = 6;
    pub const TELEPORT: u16 = 7;
    pub const SEND: u16 = 8;
    pub const LUA: u16 = 9;
    pub const PEEK: u16 = 10;
    pub const EVENTS: u16 = 11;
    pub const WORLD: u16 = 12;
    pub const BREAK: u16 = 13;
    pub const CONTROL: u16 = 14;
    pub const DEBUG: u16 = 15;
    pub const SET_CAMERA: u16 = 16;
    pub const TEXTURE: u16 = 17;
    pub const TIME_OF_DAY: u16 = 18;
    pub const SET_TIME: u16 = 19;
    pub const WEATHER: u16 = 20;
    pub const STREAM_LOAD: u16 = 21;
    pub const STREAM_UNLOAD: u16 = 22;
    pub const STREAM_QUEUE: u16 = 23;
    pub const MONEY: u16 = 24;
    pub const SET_MONEY: u16 = 25;
    pub const FAMILY: u16 = 26;
    pub const MISSION: u16 = 27;
    pub const VARIABLES: u16 = 28;
    pub const SET_VARIABLE: u16 = 29;
    pub const FILES: u16 = 30;
    pub const OBJECT: u16 = 31;
    pub const OBJECT_TRANSFORM: u16 = 32;
    pub const OBJECT_ENABLE: u16 = 33;
    pub const OBJECT_UNDO: u16 = 34;
    pub const OBJECT_EDITS: u16 = 35;
    pub const OBJECT_FORGET: u16 = 36;
    pub const PROFILE: u16 = 37;
    pub const RESOURCES: u16 = 38;
    pub const RESOURCE: u16 = 39;
}

impl Request {
    pub fn kind(&self) -> u16 {
        match self {
            Self::Hello { .. } => kind::HELLO,
            Self::Status => kind::STATUS,
            Self::Player => kind::PLAYER,
            Self::Camera => kind::CAMERA,
            Self::Streams => kind::STREAMS,
            Self::Objects { .. } => kind::OBJECTS,
            Self::Teleport { .. } => kind::TELEPORT,
            Self::Send { .. } => kind::SEND,
            Self::Lua { .. } => kind::LUA,
            Self::Peek { .. } => kind::PEEK,
            Self::Events { .. } => kind::EVENTS,
            Self::World { .. } => kind::WORLD,
            Self::Break { .. } => kind::BREAK,
            Self::Control(_) => kind::CONTROL,
            Self::Debug => kind::DEBUG,
            Self::SetCamera { .. } => kind::SET_CAMERA,
            Self::Texture { .. } => kind::TEXTURE,
            Self::TimeOfDay => kind::TIME_OF_DAY,
            Self::SetTime(_) => kind::SET_TIME,
            Self::Weather => kind::WEATHER,
            Self::StreamLoad { .. } => kind::STREAM_LOAD,
            Self::StreamUnload { .. } => kind::STREAM_UNLOAD,
            Self::StreamQueue => kind::STREAM_QUEUE,
            Self::Money => kind::MONEY,
            Self::SetMoney(_) => kind::SET_MONEY,
            Self::Family => kind::FAMILY,
            Self::Mission => kind::MISSION,
            Self::Variables { .. } => kind::VARIABLES,
            Self::SetVariable { .. } => kind::SET_VARIABLE,
            Self::Files => kind::FILES,
            Self::Object { .. } => kind::OBJECT,
            Self::ObjectTransform { .. } => kind::OBJECT_TRANSFORM,
            Self::ObjectEnable { .. } => kind::OBJECT_ENABLE,
            Self::ObjectUndo { .. } => kind::OBJECT_UNDO,
            Self::ObjectEdits => kind::OBJECT_EDITS,
            Self::ObjectForget { .. } => kind::OBJECT_FORGET,
            Self::Profile { .. } => kind::PROFILE,
            Self::Resources => kind::RESOURCES,
            Self::Resource { .. } => kind::RESOURCE,
        }
    }

    /// The first protocol version that has this request.
    pub fn since(&self) -> u16 {
        match self {
            Self::Events { .. } | Self::World { .. } => 2,
            Self::Break { .. }
            | Self::Control(_)
            | Self::Debug
            | Self::SetCamera { .. }
            | Self::Texture { .. } => 3,
            Self::TimeOfDay
            | Self::SetTime(_)
            | Self::Weather
            | Self::StreamLoad { .. }
            | Self::StreamUnload { .. }
            | Self::StreamQueue
            | Self::Money
            | Self::SetMoney(_)
            | Self::Family
            | Self::Mission
            | Self::Variables { .. }
            | Self::SetVariable { .. } => 4,
            Self::Files => 5,
            Self::Object { .. }
            | Self::ObjectTransform { .. }
            | Self::ObjectEnable { .. }
            | Self::ObjectUndo { .. }
            | Self::ObjectEdits
            | Self::ObjectForget { .. } => 6,
            Self::Profile { .. } => 7,
            Self::Resources | Self::Resource { .. } => 9,
            _ => 1,
        }
    }

    /// Whether the request changes the game.
    pub fn writes(&self) -> bool {
        matches!(
            self,
            Self::Teleport { .. }
                | Self::Send { .. }
                | Self::Lua { .. }
                | Self::SetCamera { .. }
                | Self::Texture { .. }
                | Self::SetTime(_)
                | Self::StreamLoad { .. }
                | Self::StreamUnload { .. }
                | Self::SetMoney(_)
                | Self::SetVariable { .. }
                | Self::ObjectTransform { .. }
                | Self::ObjectEnable { .. }
                | Self::ObjectUndo { .. }
        )
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::default();
        match self {
            Self::Hello { token, client } => {
                w.str(token);
                w.str(client);
            }
            Self::Status | Self::Player | Self::Camera | Self::Streams => {}
            Self::Objects {
                center,
                radius,
                class,
                max,
            } => {
                match center {
                    Some(c) => {
                        w.u8(1);
                        w.vec3(c);
                    }
                    None => w.u8(0),
                }
                w.f32(*radius);
                match class {
                    Some(c) => {
                        w.u8(1);
                        w.u32(*c);
                    }
                    None => w.u8(0),
                }
                w.u16(*max);
            }
            Self::Teleport { position, yaw } => {
                w.vec3(position);
                w.opt_f32(*yaw);
            }
            Self::Send { message, target } => {
                match message {
                    MessageId::Name(n) => {
                        w.u8(0);
                        w.str(n);
                    }
                    MessageId::Hash(h) => {
                        w.u8(1);
                        w.u32(*h);
                    }
                }
                match target {
                    Some(g) => {
                        w.u8(1);
                        w.bytes(g);
                    }
                    None => w.u8(0),
                }
            }
            Self::Lua { code } => w.str(code),
            Self::Peek { address, len } => {
                w.u32(*address);
                w.u16(*len);
            }
            Self::Events { after, kinds, max } => {
                w.u64(*after);
                w.u8(*kinds);
                w.u16(*max);
            }
            Self::World { filter, max } => {
                w.opt_u32(filter.class);
                w.u32(filter.guids.len() as u32);
                for g in &filter.guids {
                    w.bytes(g);
                }
                w.opt_u32(filter.group);
                w.u8(match filter.positioned {
                    None => 0,
                    Some(false) => 1,
                    Some(true) => 2,
                });
                w.u32(*max);
            }
            Self::Break { rules, hold_s } => {
                crate::debug::put_rules(&mut w, rules);
                w.u16(*hold_s);
            }
            Self::Control(c) => w.u8(*c as u8),
            Self::Debug => {}
            Self::SetCamera { camera } => crate::debug::put_camera(&mut w, camera),
            Self::Texture { name, data } => {
                w.u32(*name);
                w.u32(data.len() as u32);
                w.bytes(data);
            }
            Self::TimeOfDay
            | Self::Weather
            | Self::StreamQueue
            | Self::Money
            | Self::Family
            | Self::Mission
            | Self::Files
            | Self::Resources
            | Self::ObjectEdits => {}
            Self::Resource { ids } => {
                for id in ids {
                    w.u32(*id);
                }
            }
            Self::Object { guid } => w.bytes(guid),
            Self::ObjectTransform { guid, matrix } => {
                w.bytes(guid);
                crate::edit::put_matrix(&mut w, matrix);
            }
            Self::ObjectEnable {
                guid,
                enabled,
                logic,
            } => {
                w.bytes(guid);
                w.bool(*enabled);
                w.bool(*logic);
            }
            Self::Profile {
                reset,
                memory,
                sample_ms,
            } => {
                w.bool(*reset);
                w.bool(*memory);
                w.u32(*sample_ms);
            }
            Self::ObjectUndo { guid } | Self::ObjectForget { guid } => match guid {
                Some(g) => {
                    w.u8(1);
                    w.bytes(g);
                }
                None => w.u8(0),
            },
            Self::SetTime(c) => crate::state::put_time_change(&mut w, c),
            Self::StreamLoad { name } => w.str(name),
            Self::StreamUnload { name, force } => {
                w.str(name);
                w.bool(*force);
            }
            Self::SetMoney(c) => crate::state::put_money_change(&mut w, c),
            Self::Variables { hashes } => {
                w.u32(hashes.len() as u32);
                for h in hashes {
                    w.u32(*h);
                }
            }
            Self::SetVariable { hash, value } => {
                w.u32(*hash);
                w.u32(*value as u32);
            }
        }
        w.buf
    }

    pub fn decode(kind: u16, payload: &[u8]) -> Result<Self, DecodeError> {
        let mut r = Reader::new(payload);
        let out = match kind {
            kind::HELLO => Self::Hello {
                token: r.str()?,
                client: r.str()?,
            },
            kind::STATUS => Self::Status,
            kind::PLAYER => Self::Player,
            kind::CAMERA => Self::Camera,
            kind::STREAMS => Self::Streams,
            kind::OBJECTS => Self::Objects {
                center: if r.bool()? { Some(r.vec3()?) } else { None },
                radius: r.f32()?,
                class: if r.bool()? { Some(r.u32()?) } else { None },
                max: r.u16()?,
            },
            kind::TELEPORT => Self::Teleport {
                position: r.vec3()?,
                yaw: r.opt_f32()?,
            },
            kind::SEND => Self::Send {
                message: match r.u8()? {
                    0 => MessageId::Name(r.str()?),
                    1 => MessageId::Hash(r.u32()?),
                    _ => return Err(DecodeError::BadValue("message")),
                },
                target: if r.bool()? { Some(r.guid()?) } else { None },
            },
            kind::LUA => Self::Lua { code: r.str()? },
            kind::PEEK => Self::Peek {
                address: r.u32()?,
                len: r.u16()?,
            },
            kind::EVENTS => Self::Events {
                after: r.u64()?,
                kinds: r.u8()?,
                max: r.u16()?,
            },
            kind::WORLD => Self::World {
                filter: WorldFilter {
                    class: r.opt_u32()?,
                    guids: {
                        let n = r.count(16)?;
                        let mut list = Vec::with_capacity(n);
                        for _ in 0..n {
                            list.push(r.guid()?);
                        }
                        list
                    },
                    group: r.opt_u32()?,
                    positioned: match r.u8()? {
                        0 => None,
                        1 => Some(false),
                        2 => Some(true),
                        _ => return Err(DecodeError::BadValue("positioned")),
                    },
                },
                max: r.u32()?,
            },
            kind::BREAK => Self::Break {
                rules: crate::debug::get_rules(&mut r)?,
                hold_s: r.u16()?,
            },
            kind::CONTROL => {
                Self::Control(Control::from_u8(r.u8()?).ok_or(DecodeError::BadValue("control"))?)
            }
            kind::DEBUG => Self::Debug,
            kind::SET_CAMERA => Self::SetCamera {
                camera: crate::debug::get_camera(&mut r)?,
            },
            kind::TEXTURE => Self::Texture {
                name: r.u32()?,
                data: {
                    let n = r.count(1)?;
                    r.take(n)?.to_vec()
                },
            },
            kind::TIME_OF_DAY => Self::TimeOfDay,
            kind::SET_TIME => Self::SetTime(crate::state::get_time_change(&mut r)?),
            kind::WEATHER => Self::Weather,
            kind::STREAM_LOAD => Self::StreamLoad { name: r.str()? },
            kind::STREAM_UNLOAD => Self::StreamUnload {
                name: r.str()?,
                force: r.bool()?,
            },
            kind::STREAM_QUEUE => Self::StreamQueue,
            kind::MONEY => Self::Money,
            kind::SET_MONEY => Self::SetMoney(crate::state::get_money_change(&mut r)?),
            kind::FAMILY => Self::Family,
            kind::MISSION => Self::Mission,
            kind::VARIABLES => Self::Variables {
                hashes: {
                    let n = r.count(4)?;
                    let mut list = Vec::with_capacity(n);
                    for _ in 0..n {
                        list.push(r.u32()?);
                    }
                    list
                },
            },
            kind::SET_VARIABLE => Self::SetVariable {
                hash: r.u32()?,
                value: r.u32()? as i32,
            },
            kind::FILES => Self::Files,
            kind::RESOURCES => Self::Resources,
            kind::RESOURCE => Self::Resource {
                ids: [r.u32()?, r.u32()?, r.u32()?, r.u32()?],
            },
            kind::OBJECT => Self::Object { guid: r.guid()? },
            kind::OBJECT_TRANSFORM => Self::ObjectTransform {
                guid: r.guid()?,
                matrix: crate::edit::get_matrix(&mut r)?,
            },
            kind::OBJECT_ENABLE => Self::ObjectEnable {
                guid: r.guid()?,
                enabled: r.bool()?,
                logic: r.bool()?,
            },
            kind::OBJECT_UNDO => Self::ObjectUndo {
                guid: if r.bool()? { Some(r.guid()?) } else { None },
            },
            kind::OBJECT_EDITS => Self::ObjectEdits,
            kind::OBJECT_FORGET => Self::ObjectForget {
                guid: if r.bool()? { Some(r.guid()?) } else { None },
            },
            kind::PROFILE => Self::Profile {
                reset: r.bool()?,
                memory: r.bool()?,
                sample_ms: r.u32()?,
            },
            other => return Err(DecodeError::UnknownKind(other)),
        };
        r.finish()?;
        Ok(out)
    }

    /// The whole frame.
    pub fn to_frame(&self, id: u32) -> Vec<u8> {
        frame(self.kind(), id, &self.encode())
    }

    /// The whole frame in protocol `version`.
    pub fn to_frame_of(&self, version: u16, id: u32) -> Vec<u8> {
        frame_of(version, self.kind(), id, &self.encode())
    }
}

/// Answer to [`Request::Hello`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hello {
    pub protocol: u16,
    /// Version of the module (`doctor_live 0.1.0`).
    pub module: String,
    /// Whether the game is the build the module knows ([`crate::edition`]).
    pub edition_ok: bool,
    /// The edition, or why it is not the known one.
    pub edition: String,
    /// Whether the configuration allows writes.
    pub writes: bool,
}

/// State of the game's main loop (`godfather2.exe` `0x0112980C`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoopState {
    Unknown = 0,
    /// `InitOnceMainLoop`: start-up.
    Init = 1,
    /// `FrontendMainLoop`: menus.
    Frontend = 2,
    /// `GameMainLoop`: a level.
    Game = 3,
}

impl LoopState {
    pub fn from_u8(v: u8) -> Self {
        match v {
            1 => Self::Init,
            2 => Self::Frontend,
            3 => Self::Game,
            _ => Self::Unknown,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Init => "InitOnceMainLoop",
            Self::Frontend => "FrontendMainLoop",
            Self::Game => "GameMainLoop",
        }
    }
}

/// Answer to [`Request::Status`].
#[derive(Debug, Clone, PartialEq)]
pub struct Status {
    /// Frames of the main loop since the module was loaded.
    pub frame: u32,
    pub state: LoopState,
    /// Whether the local player exists (a level is loaded and he is in it).
    pub player: bool,
    /// Simulation time of the game in milliseconds (`GetSimTime` of Lua).
    pub sim_time_ms: Option<u32>,
    /// Time of day in the game: hours, minutes, seconds.
    pub time_of_day: Option<[u8; 3]>,
}

/// Answer to [`Request::Player`]: the player's world matrix.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pose {
    pub position: [f32; 3],
    pub right: [f32; 3],
    pub up: [f32; 3],
    pub forward: [f32; 3],
}

impl Pose {
    /// Turn about the vertical axis in radians, 0 when facing +Z
    /// (`atan2(forward.x, forward.z)`).
    pub fn yaw(&self) -> f32 {
        self.forward[0].atan2(self.forward[2])
    }
}

/// Answer to [`Request::Camera`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CameraView {
    pub pose: Pose,
    /// Vertical field of view in degrees, when known.
    pub fov: Option<f32>,
}

/// A loaded stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamInfo {
    pub name: String,
}

/// An object found by [`Request::Objects`].
#[derive(Debug, Clone, PartialEq)]
pub struct ObjectInfo {
    pub guid: Guid,
    /// Hash of the class name ([`crate::name_hash`]); 0 when unknown.
    pub class: u32,
    /// The class name as the game registered it (`VariableSwitch`).
    pub class_name: String,
    pub position: [f32; 3],
    pub distance: f32,
}

/// Answer to a write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Done {
    /// How many objects got the message (for [`Request::Send`]).
    pub count: u32,
    pub note: String,
}

/// Answer to [`Request::Lua`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LuaReply {
    /// `false`: the code did not compile or failed; `values` holds the
    /// message.
    pub ok: bool,
    pub values: Vec<String>,
}

/// Why a request failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCode {
    /// The request cannot be read.
    BadRequest = 1,
    /// Another protocol version.
    Version = 2,
    /// The game is not the build the module knows.
    Edition = 3,
    /// Not now: no level, no player, the game is loading.
    NotReady = 4,
    /// The game did not reach a safe point in time (paused, minimised).
    Timeout = 5,
    /// Wrong token, writes switched off, or Hello first.
    Denied = 6,
    /// The game refused or the call failed.
    Failed = 7,
    /// Not supported by this module.
    Unsupported = 8,
}

impl ErrorCode {
    pub fn from_u16(v: u16) -> Self {
        match v {
            1 => Self::BadRequest,
            2 => Self::Version,
            3 => Self::Edition,
            4 => Self::NotReady,
            5 => Self::Timeout,
            6 => Self::Denied,
            8 => Self::Unsupported,
            _ => Self::Failed,
        }
    }
}

/// An error reply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    pub code: ErrorCode,
    pub message: String,
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}: {}", self.code, self.message)
    }
}

/// A reply of the module.
#[derive(Debug, Clone, PartialEq)]
pub enum Response {
    Hello(Hello),
    Status(Status),
    /// `None`: no player now.
    Player(Option<Pose>),
    /// `None`: no camera now.
    Camera(Option<CameraView>),
    Streams(Vec<StreamInfo>),
    Objects(Vec<ObjectInfo>),
    /// Answer to [`Request::Teleport`] and [`Request::Send`].
    Done(Done),
    Lua(LuaReply),
    Peek(Vec<u8>),
    /// Version 2: answer to [`Request::Events`].
    Events(EventBatch),
    /// Version 2: answer to [`Request::World`].
    World(World),
    /// Version 3: answer to [`Request::Debug`].
    Debug(DebugState),
    /// Version 4: answer to [`Request::TimeOfDay`] (`None`: no clock now).
    TimeOfDay(Option<TimeOfDay>),
    /// Version 4: answer to [`Request::Weather`].
    Weather(Weather),
    /// Version 4: answer to [`Request::StreamQueue`].
    StreamQueue(StreamQueue),
    /// Version 4: answer to [`Request::Money`] (`None`: no family of the
    /// player now).
    Money(Option<Money>),
    /// Version 4: answer to [`Request::Family`].
    Family(Family),
    /// Version 4: answer to [`Request::Mission`].
    Mission(Mission),
    /// Version 4: answer to [`Request::Variables`].
    Variables(Vec<Variable>),
    /// Version 5: answer to [`Request::Files`].
    Files(Files),
    /// Version 6: answer to [`Request::Object`].
    Object(ObjectState),
    /// Version 6: answer to [`Request::ObjectTransform`],
    /// [`Request::ObjectEnable`] and [`Request::ObjectUndo`] — the edit made
    /// (for an undo: the change back, under the number of the edit undone).
    ObjectEdit(ObjectEdit),
    /// Version 6: answer to [`Request::ObjectEdits`].
    ObjectEdits(Vec<ObjectEdit>),
    /// Version 7: answer to [`Request::Profile`].
    Profile(Box<Profile>),
    /// Version 9: answer to [`Request::Resources`].
    Resources(Resources),
    /// Version 9: answer to [`Request::Resource`]; `None` when no loaded
    /// resource has those ids.
    Resource(Option<Loaded>),
    Error(Failure),
}

fn put_pose(w: &mut Writer, p: &Pose) {
    w.vec3(&p.position);
    w.vec3(&p.right);
    w.vec3(&p.up);
    w.vec3(&p.forward);
}

fn get_pose(r: &mut Reader<'_>) -> Result<Pose, DecodeError> {
    Ok(Pose {
        position: r.vec3()?,
        right: r.vec3()?,
        up: r.vec3()?,
        forward: r.vec3()?,
    })
}

impl Response {
    /// Kind of the frame for an answer to a request of `request` kind.
    pub fn kind(&self, request: u16) -> u16 {
        match self {
            Self::Error(_) => ERROR_KIND,
            _ => request | REPLY,
        }
    }

    pub fn error(code: ErrorCode, message: impl Into<String>) -> Self {
        Self::Error(Failure {
            code,
            message: message.into(),
        })
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::default();
        match self {
            Self::Hello(h) => {
                w.u16(h.protocol);
                w.str(&h.module);
                w.bool(h.edition_ok);
                w.str(&h.edition);
                w.bool(h.writes);
            }
            Self::Status(s) => {
                w.u32(s.frame);
                w.u8(s.state as u8);
                w.bool(s.player);
                match s.sim_time_ms {
                    Some(t) => {
                        w.u8(1);
                        w.u32(t);
                    }
                    None => w.u8(0),
                }
                match s.time_of_day {
                    Some(t) => {
                        w.u8(1);
                        w.bytes(&t);
                    }
                    None => w.u8(0),
                }
            }
            Self::Player(p) => match p {
                Some(p) => {
                    w.u8(1);
                    put_pose(&mut w, p);
                }
                None => w.u8(0),
            },
            Self::Camera(c) => match c {
                Some(c) => {
                    w.u8(1);
                    put_pose(&mut w, &c.pose);
                    w.opt_f32(c.fov);
                }
                None => w.u8(0),
            },
            Self::Streams(list) => {
                w.u32(list.len() as u32);
                for s in list {
                    w.str(&s.name);
                }
            }
            Self::Objects(list) => {
                w.u32(list.len() as u32);
                for o in list {
                    w.bytes(&o.guid);
                    w.u32(o.class);
                    w.str(&o.class_name);
                    w.vec3(&o.position);
                    w.f32(o.distance);
                }
            }
            Self::Done(d) => {
                w.u32(d.count);
                w.str(&d.note);
            }
            Self::Lua(l) => {
                w.bool(l.ok);
                w.u32(l.values.len() as u32);
                for v in &l.values {
                    w.str(v);
                }
            }
            Self::Peek(bytes) => {
                w.u32(bytes.len() as u32);
                w.bytes(bytes);
            }
            Self::Events(b) => put_events(&mut w, b),
            Self::World(world) => put_world(&mut w, world),
            Self::Debug(d) => crate::debug::put_state(&mut w, d),
            Self::TimeOfDay(t) => crate::state::put_time(&mut w, t),
            Self::Weather(x) => crate::state::put_weather(&mut w, x),
            Self::StreamQueue(q) => crate::state::put_stream_queue(&mut w, q),
            Self::Money(m) => crate::state::put_money(&mut w, m),
            Self::Family(f) => crate::state::put_family(&mut w, f),
            Self::Mission(m) => crate::state::put_mission(&mut w, m),
            Self::Variables(v) => crate::state::put_variables(&mut w, v),
            Self::Files(f) => crate::files::put_files(&mut w, f),
            Self::Object(s) => crate::edit::put_state(&mut w, s),
            Self::ObjectEdit(e) => crate::edit::put_edit(&mut w, e),
            Self::ObjectEdits(list) => crate::edit::put_edits(&mut w, list),
            Self::Profile(p) => crate::profile::put_profile(&mut w, p),
            Self::Resources(r) => crate::resources::put_resources(&mut w, r),
            Self::Resource(l) => crate::resources::put_loaded(&mut w, l),
            Self::Error(e) => {
                w.u16(e.code as u16);
                w.str(&e.message);
            }
        }
        w.buf
    }

    /// Reads the reply of frame kind `kind`.
    pub fn decode(kind: u16, payload: &[u8]) -> Result<Self, DecodeError> {
        let mut r = Reader::new(payload);
        let out = if kind == ERROR_KIND {
            Self::Error(Failure {
                code: ErrorCode::from_u16(r.u16()?),
                message: r.str()?,
            })
        } else {
            if kind & REPLY == 0 {
                return Err(DecodeError::UnknownKind(kind));
            }
            match kind & !REPLY {
                kind::HELLO => Self::Hello(Hello {
                    protocol: r.u16()?,
                    module: r.str()?,
                    edition_ok: r.bool()?,
                    edition: r.str()?,
                    writes: r.bool()?,
                }),
                kind::STATUS => Self::Status(Status {
                    frame: r.u32()?,
                    state: LoopState::from_u8(r.u8()?),
                    player: r.bool()?,
                    sim_time_ms: if r.bool()? { Some(r.u32()?) } else { None },
                    time_of_day: if r.bool()? {
                        Some([r.u8()?, r.u8()?, r.u8()?])
                    } else {
                        None
                    },
                }),
                kind::PLAYER => Self::Player(if r.bool()? {
                    Some(get_pose(&mut r)?)
                } else {
                    None
                }),
                kind::CAMERA => Self::Camera(if r.bool()? {
                    Some(CameraView {
                        pose: get_pose(&mut r)?,
                        fov: r.opt_f32()?,
                    })
                } else {
                    None
                }),
                kind::STREAMS => {
                    let n = r.count(2)?;
                    let mut list = Vec::with_capacity(n);
                    for _ in 0..n {
                        list.push(StreamInfo { name: r.str()? });
                    }
                    Self::Streams(list)
                }
                kind::OBJECTS => {
                    let n = r.count(38)?;
                    let mut list = Vec::with_capacity(n);
                    for _ in 0..n {
                        list.push(ObjectInfo {
                            guid: r.guid()?,
                            class: r.u32()?,
                            class_name: r.str()?,
                            position: r.vec3()?,
                            distance: r.f32()?,
                        });
                    }
                    Self::Objects(list)
                }
                kind::TELEPORT
                | kind::SEND
                | kind::BREAK
                | kind::CONTROL
                | kind::SET_CAMERA
                | kind::TEXTURE
                | kind::SET_TIME
                | kind::STREAM_LOAD
                | kind::STREAM_UNLOAD
                | kind::SET_MONEY
                | kind::SET_VARIABLE
                | kind::OBJECT_FORGET => Self::Done(Done {
                    count: r.u32()?,
                    note: r.str()?,
                }),
                kind::LUA => {
                    let ok = r.bool()?;
                    let n = r.count(2)?;
                    let mut values = Vec::with_capacity(n);
                    for _ in 0..n {
                        values.push(r.str()?);
                    }
                    Self::Lua(LuaReply { ok, values })
                }
                kind::PEEK => {
                    let n = r.count(1)?;
                    Self::Peek(r.take(n)?.to_vec())
                }
                kind::EVENTS => Self::Events(get_events(&mut r)?),
                kind::WORLD => Self::World(get_world(&mut r)?),
                kind::DEBUG => Self::Debug(crate::debug::get_state(&mut r)?),
                kind::TIME_OF_DAY => Self::TimeOfDay(crate::state::get_time(&mut r)?),
                kind::WEATHER => Self::Weather(crate::state::get_weather(&mut r)?),
                kind::STREAM_QUEUE => Self::StreamQueue(crate::state::get_stream_queue(&mut r)?),
                kind::MONEY => Self::Money(crate::state::get_money(&mut r)?),
                kind::FAMILY => Self::Family(crate::state::get_family(&mut r)?),
                kind::MISSION => Self::Mission(crate::state::get_mission(&mut r)?),
                kind::VARIABLES => Self::Variables(crate::state::get_variables(&mut r)?),
                kind::FILES => Self::Files(crate::files::get_files(&mut r)?),
                kind::OBJECT => Self::Object(crate::edit::get_state(&mut r)?),
                kind::OBJECT_TRANSFORM | kind::OBJECT_ENABLE | kind::OBJECT_UNDO => {
                    Self::ObjectEdit(crate::edit::get_edit(&mut r)?)
                }
                kind::OBJECT_EDITS => Self::ObjectEdits(crate::edit::get_edits(&mut r)?),
                kind::PROFILE => Self::Profile(Box::new(crate::profile::get_profile(&mut r)?)),
                kind::RESOURCES => Self::Resources(crate::resources::get_resources(&mut r)?),
                kind::RESOURCE => Self::Resource(crate::resources::get_loaded(&mut r)?),
                other => return Err(DecodeError::UnknownKind(other | REPLY)),
            }
        };
        r.finish()?;
        Ok(out)
    }

    /// The whole frame answering request `id` of kind `request`.
    pub fn to_frame(&self, request: u16, id: u32) -> Vec<u8> {
        frame(self.kind(request), id, &self.encode())
    }

    /// The same in protocol `version` (that of the request).
    pub fn to_frame_of(&self, version: u16, request: u16, id: u32) -> Vec<u8> {
        frame_of(version, self.kind(request), id, &self.encode())
    }
}

fn put_events(w: &mut Writer, b: &EventBatch) {
    w.u64(b.next);
    w.u64(b.lost);
    w.u8(b.recording);
    w.u32(b.events.len() as u32);
    for e in &b.events {
        w.u64(e.seq);
        w.u32(e.frame);
        w.u32(e.sim_time_ms);
        match &e.body {
            EventBody::Output {
                source,
                text,
                script,
                line,
            } => {
                w.u8(1);
                w.u8(*source as u8);
                w.str(text);
                w.str(script);
                w.u32(*line as u32);
            }
            EventBody::LuaError {
                status,
                message,
                chunk,
                line,
                traceback,
            } => {
                w.u8(2);
                w.u8(*status);
                w.str(message);
                w.str(chunk);
                w.u32(*line as u32);
                w.u32(traceback.len() as u32);
                for t in traceback {
                    w.str(t);
                }
            }
            EventBody::Message {
                hash,
                data,
                forced,
                listeners,
                receivers,
                origin,
                from,
            } => {
                // 3 is the event of protocol 2; 5 (protocol 8) carries the
                // origin after the same fields.
                w.u8(if from.is_some() { 5 } else { 3 });
                w.u32(*hash);
                w.u32(*data);
                w.bool(*forced);
                w.u32(*listeners);
                w.u32(receivers.len() as u32);
                for g in receivers {
                    w.bytes(g);
                }
                w.u8(*origin as u8);
                if let Some(f) = from {
                    w.u32(f.site);
                    w.u32(f.cause);
                    w.u8(u8::from(f.sender.is_some())
                        | u8::from(f.data_object.is_some()) << 1
                        | u8::from(f.direct) << 2);
                    if let Some(g) = &f.sender {
                        w.bytes(g);
                    }
                    if let Some(g) = &f.data_object {
                        w.bytes(g);
                    }
                }
            }
            EventBody::MessageCounts { window_ms, counts } => {
                w.u8(4);
                w.u32(*window_ms);
                w.u32(counts.len() as u32);
                for (h, n) in counts {
                    w.u32(*h);
                    w.u32(*n);
                }
            }
        }
    }
}

fn get_events(r: &mut Reader<'_>) -> Result<EventBatch, DecodeError> {
    let next = r.u64()?;
    let lost = r.u64()?;
    let recording = r.u8()?;
    let n = r.count(17)?;
    let mut events = Vec::with_capacity(n);
    for _ in 0..n {
        let seq = r.u64()?;
        let frame = r.u32()?;
        let sim_time_ms = r.u32()?;
        let body = match r.u8()? {
            1 => EventBody::Output {
                source: OutputSource::from_u8(r.u8()?),
                text: r.str()?,
                script: r.str()?,
                line: r.u32()? as i32,
            },
            2 => EventBody::LuaError {
                status: r.u8()?,
                message: r.str()?,
                chunk: r.str()?,
                line: r.u32()? as i32,
                traceback: {
                    let n = r.count(2)?;
                    let mut lines = Vec::with_capacity(n);
                    for _ in 0..n {
                        lines.push(r.str()?);
                    }
                    lines
                },
            },
            kind @ (3 | 5) => EventBody::Message {
                hash: r.u32()?,
                data: r.u32()?,
                forced: r.bool()?,
                listeners: r.u32()?,
                receivers: {
                    let n = r.count(16)?;
                    let mut list = Vec::with_capacity(n);
                    for _ in 0..n {
                        list.push(r.guid()?);
                    }
                    list
                },
                origin: match r.u8()? {
                    0 => MessageOrigin::Game,
                    1 => MessageOrigin::Link,
                    _ => return Err(DecodeError::BadValue("message origin")),
                },
                from: if kind == 5 {
                    let site = r.u32()?;
                    let cause = r.u32()?;
                    let flags = r.u8()?;
                    if flags > 7 {
                        return Err(DecodeError::BadValue("message origin flags"));
                    }
                    Some(MessageFrom {
                        site,
                        cause,
                        sender: if flags & 1 != 0 {
                            Some(r.guid()?)
                        } else {
                            None
                        },
                        data_object: if flags & 2 != 0 {
                            Some(r.guid()?)
                        } else {
                            None
                        },
                        direct: flags & 4 != 0,
                    })
                } else {
                    None
                },
            },
            4 => EventBody::MessageCounts {
                window_ms: r.u32()?,
                counts: {
                    let n = r.count(8)?;
                    let mut list = Vec::with_capacity(n);
                    for _ in 0..n {
                        list.push((r.u32()?, r.u32()?));
                    }
                    list
                },
            },
            _ => return Err(DecodeError::BadValue("event")),
        };
        events.push(Event {
            seq,
            frame,
            sim_time_ms,
            body,
        });
    }
    Ok(EventBatch {
        next,
        lost,
        recording,
        events,
    })
}

fn put_world(w: &mut Writer, world: &World) {
    w.u32(world.total);
    w.u32(world.walked);
    w.bool(world.partial);
    w.u32(world.classes.len() as u32);
    for (h, n) in &world.classes {
        w.u32(*h);
        w.str(n);
    }
    w.u32(world.streams.len() as u32);
    for (g, n) in &world.streams {
        w.u32(*g);
        w.str(n);
    }
    w.u32(world.objects.len() as u32);
    for o in &world.objects {
        w.bytes(&o.guid);
        w.u32(o.class);
        w.u32(o.group);
        match &o.position {
            Some(p) => {
                w.u8(1);
                w.vec3(p);
            }
            None => w.u8(0),
        }
        w.u32(o.flags);
        w.bool(o.listening);
    }
}

fn get_world(r: &mut Reader<'_>) -> Result<World, DecodeError> {
    let total = r.u32()?;
    let walked = r.u32()?;
    let partial = r.bool()?;
    let n = r.count(6)?;
    let mut classes = Vec::with_capacity(n);
    for _ in 0..n {
        classes.push((r.u32()?, r.str()?));
    }
    let n = r.count(6)?;
    let mut streams = Vec::with_capacity(n);
    for _ in 0..n {
        streams.push((r.u32()?, r.str()?));
    }
    let n = r.count(30)?;
    let mut objects = Vec::with_capacity(n);
    for _ in 0..n {
        objects.push(WorldObject {
            guid: r.guid()?,
            class: r.u32()?,
            group: r.u32()?,
            position: if r.bool()? { Some(r.vec3()?) } else { None },
            flags: r.u32()?,
            listening: r.bool()?,
        });
    }
    Ok(World {
        total,
        walked,
        partial,
        classes,
        streams,
        objects,
    })
}

#[cfg(test)]
mod tests;

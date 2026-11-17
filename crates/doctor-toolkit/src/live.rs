//! Live link with the running game (stage 34): the editor's side.
//!
//! - [`Client`]: a connection to the module in the game
//!   (`127.0.0.1:PORT`, protocol [`proto`]);
//! - [`loader`]: whether the game folder has an ASI loader that will load
//!   the module;
//! - [`ModuleInstall`]: puts the module (`scripts/doctor_live.asi`) and its
//!   configuration into a game folder for one launch and takes them away
//!   afterwards, like `cmd.txt` ([`crate::play::start::CmdFile`]);
//! - [`fake`]: a server that answers like the module, for tests and for
//!   trying the editor without the game;
//! - [`commit`]: objects moved in the running game written into the
//!   workspace as SimGroup records (stage 47);
//! - [`profile`]: the profile of the game measured by the module (stage
//!   58) as text and JSON;
//! - [`names`]: names, classes and streams of SimGroup objects by GUID and
//!   names of messages by hash, from the SimGroups of the game and its
//!   workspace (stage 49: the journal and every object).
//!
//! See `docs/analysis/live-link.md`.

use std::io::Write as _;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, TcpStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

pub use doctor_toolkit_live as proto;
pub use doctor_toolkit_live::config::Config;
use doctor_toolkit_live::{
    BreakRule, CONFIG_FILE, CameraView, Control, DebugState, Done, ErrorCode, EventBatch, Failure,
    Family, Hello, LOG_FILE, LuaReply, MODULE_FILE, MessageId, Mission, Money, MoneyChange,
    OLDEST_PROTOCOL_VERSION, ObjectInfo, PROTOCOL_VERSION, Pose, Request, Response, Status,
    StreamInfo, StreamQueue, TimeChange, TimeOfDay, Variable, Weather, World, WorldFilter,
    read_frame,
};

use crate::{Error, Result};

pub mod commit;
pub mod fake;
pub mod loader;
pub mod names;
pub mod profile;
pub mod view;

/// The live link in the options of Play (`settings.toml`, `play.live`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct LiveOptions {
    /// Install the module for each launch and take it away afterwards.
    pub enabled: bool,
    /// Allow teleport, messages and Lua (the editor still asks each time).
    pub writes: bool,
}

impl LiveOptions {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// The configuration of one launch: a free port and a new token.
    pub fn config(&self) -> Config {
        Config {
            port: free_port(),
            token: new_token(),
            writes: self.writes,
            fix_input: true,
            journal: true,
            ..Default::default()
        }
    }
}

/// Folder of the game where the ASI loader looks for modules.
pub const SCRIPTS_DIR: &str = "scripts";

/// What went wrong talking to the module.
#[derive(Debug)]
pub enum LinkError {
    /// No module listens: the game does not run, or runs without it.
    Unreachable(std::io::Error),
    /// The connection broke or a frame could not be read.
    Io(std::io::Error),
    /// The module answered with an error.
    Refused(Failure),
    /// An answer of the wrong kind.
    Unexpected(String),
}

impl std::fmt::Display for LinkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unreachable(e) => write!(f, "no live link: {e}"),
            Self::Io(e) => write!(f, "live link broken: {e}"),
            Self::Refused(e) => write!(f, "{}", e.message),
            Self::Unexpected(what) => write!(f, "unexpected answer: {what}"),
        }
    }
}

impl std::error::Error for LinkError {}

impl LinkError {
    /// The error code of a refusal.
    pub fn code(&self) -> Option<ErrorCode> {
        match self {
            Self::Refused(f) => Some(f.code),
            _ => None,
        }
    }
}

pub type LinkResult<T> = std::result::Result<T, LinkError>;

/// Where to connect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    pub port: u16,
    pub token: String,
}

impl Default for Endpoint {
    fn default() -> Self {
        Self {
            port: proto::DEFAULT_PORT,
            token: String::new(),
        }
    }
}

impl Endpoint {
    /// The port and token of the module installed in a game folder
    /// (`scripts/doctor_live.cfg`), else the defaults.
    pub fn of_game(root: &Path) -> Self {
        match std::fs::read_to_string(root.join(SCRIPTS_DIR).join(CONFIG_FILE)) {
            Ok(text) => {
                let c = Config::parse(&text);
                Self {
                    port: c.port,
                    token: c.token,
                }
            }
            Err(_) => Self::default(),
        }
    }

    pub fn address(&self) -> SocketAddr {
        SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, self.port))
    }
}

/// A connection to the module.
pub struct Client {
    stream: TcpStream,
    next_id: u32,
    hello: Hello,
    /// Protocol version of the frames: [`PROTOCOL_VERSION`], or 1 with a
    /// module of stage 34.
    version: u16,
}

impl Client {
    /// Connects and says Hello. `timeout` bounds the connect and every
    /// answer (requests that wait for the game's main loop can take a
    /// while at 1–2 frames a second). A module that speaks only protocol 1
    /// (stage 34) refuses the first Hello and closes: then the client
    /// connects again with version 1 and does without the journal and the
    /// list of every object. A module of protocol 2 (stage 49) the same
    /// way: the client tries the versions from the newest down.
    pub fn connect(endpoint: &Endpoint, client: &str, timeout: Duration) -> LinkResult<Self> {
        let mut first = None;
        for version in (OLDEST_PROTOCOL_VERSION..=PROTOCOL_VERSION).rev() {
            match Self::connect_with(endpoint, client, timeout, version) {
                Err(e) if e.code() == Some(ErrorCode::Version) => {
                    first.get_or_insert(e);
                }
                // A first refusal for a reason of its own stands; after a
                // version refusal, an error of an older try does not hide it.
                Err(e) if first.is_some() && e.code().is_none() => {
                    return Err(first.unwrap_or(e));
                }
                other => return other,
            }
        }
        Err(first.unwrap_or_else(|| {
            LinkError::Unexpected("no protocol version was accepted".to_owned())
        }))
    }

    fn connect_with(
        endpoint: &Endpoint,
        client: &str,
        timeout: Duration,
        version: u16,
    ) -> LinkResult<Self> {
        let stream = TcpStream::connect_timeout(&endpoint.address(), timeout)
            .map_err(LinkError::Unreachable)?;
        stream
            .set_read_timeout(Some(timeout))
            .map_err(LinkError::Io)?;
        stream
            .set_write_timeout(Some(timeout))
            .map_err(LinkError::Io)?;
        let _ = stream.set_nodelay(true);
        let mut this = Self {
            stream,
            next_id: 1,
            hello: Hello {
                protocol: version,
                module: String::new(),
                edition_ok: false,
                edition: String::new(),
                writes: false,
            },
            version,
        };
        match this.request(&Request::Hello {
            token: endpoint.token.clone(),
            client: client.to_owned(),
        })? {
            Response::Hello(h) => this.hello = h,
            other => return Err(LinkError::Unexpected(format!("{other:?}"))),
        }
        Ok(this)
    }

    /// What the module said in Hello.
    pub fn hello(&self) -> &Hello {
        &self.hello
    }

    /// Protocol version of this connection.
    pub fn version(&self) -> u16 {
        self.version
    }

    /// Whether the module has the journal and the list of every object
    /// (protocol 2, stage 49).
    pub fn has_journal(&self) -> bool {
        self.version >= 2
    }

    /// Whether the module has breakpoints, pause, the free camera and the
    /// texture replacement (protocol 3, stage 35).
    pub fn has_debug(&self) -> bool {
        self.version >= 3
    }

    /// Changes how long answers may take.
    pub fn set_timeout(&self, timeout: Duration) {
        let _ = self.stream.set_read_timeout(Some(timeout));
    }

    /// Sends one request and reads its answer; an error answer becomes
    /// [`LinkError::Refused`].
    pub fn request(&mut self, request: &Request) -> LinkResult<Response> {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        if request.since() > self.version {
            return Err(LinkError::Refused(Failure {
                code: ErrorCode::Unsupported,
                message: format!(
                    "the module in the game is older (protocol {}): install the module of this toolkit for this",
                    self.version
                ),
            }));
        }
        self.stream
            .write_all(&request.to_frame_of(self.version, id))
            .map_err(LinkError::Io)?;
        let (header, payload) = read_frame(&mut self.stream).map_err(LinkError::Io)?;
        if !proto::supported(header.version) {
            return Err(LinkError::Refused(Failure {
                code: ErrorCode::Version,
                message: format!(
                    "the module speaks protocol {}, the toolkit {OLDEST_PROTOCOL_VERSION} to {PROTOCOL_VERSION}",
                    header.version
                ),
            }));
        }
        if header.id != id {
            return Err(LinkError::Unexpected(format!(
                "answer {} to request {id}",
                header.id
            )));
        }
        let response = Response::decode(header.kind, &payload)
            .map_err(|e| LinkError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, e)))?;
        match response {
            Response::Error(f) => Err(LinkError::Refused(f)),
            r => Ok(r),
        }
    }

    pub fn status(&mut self) -> LinkResult<Status> {
        match self.request(&Request::Status)? {
            Response::Status(s) => Ok(s),
            other => Err(unexpected(other)),
        }
    }

    pub fn player(&mut self) -> LinkResult<Option<Pose>> {
        match self.request(&Request::Player)? {
            Response::Player(p) => Ok(p),
            other => Err(unexpected(other)),
        }
    }

    pub fn camera(&mut self) -> LinkResult<Option<CameraView>> {
        match self.request(&Request::Camera)? {
            Response::Camera(c) => Ok(c),
            other => Err(unexpected(other)),
        }
    }

    pub fn streams(&mut self) -> LinkResult<Vec<StreamInfo>> {
        match self.request(&Request::Streams)? {
            Response::Streams(s) => Ok(s),
            other => Err(unexpected(other)),
        }
    }

    pub fn objects(
        &mut self,
        center: Option<[f32; 3]>,
        radius: f32,
        class: Option<u32>,
        max: u16,
    ) -> LinkResult<Vec<ObjectInfo>> {
        match self.request(&Request::Objects {
            center,
            radius,
            class,
            max,
        })? {
            Response::Objects(o) => Ok(o),
            other => Err(unexpected(other)),
        }
    }

    pub fn teleport(&mut self, position: [f32; 3], yaw: Option<f32>) -> LinkResult<Done> {
        match self.request(&Request::Teleport { position, yaw })? {
            Response::Done(d) => Ok(d),
            other => Err(unexpected(other)),
        }
    }

    pub fn send(&mut self, message: MessageId, target: Option<proto::Guid>) -> LinkResult<Done> {
        match self.request(&Request::Send { message, target })? {
            Response::Done(d) => Ok(d),
            other => Err(unexpected(other)),
        }
    }

    pub fn lua(&mut self, code: &str) -> LinkResult<LuaReply> {
        match self.request(&Request::Lua {
            code: code.to_owned(),
        })? {
            Response::Lua(l) => Ok(l),
            other => Err(unexpected(other)),
        }
    }

    pub fn peek(&mut self, address: u32, len: u16) -> LinkResult<Vec<u8>> {
        match self.request(&Request::Peek { address, len })? {
            Response::Peek(b) => Ok(b),
            other => Err(unexpected(other)),
        }
    }

    /// Events of the game's journal after number `after` whose kind is in
    /// `kinds` ([`proto::journal::mask`]). Asking for messages keeps their
    /// recording on in the module for a while.
    pub fn events(&mut self, after: u64, kinds: u8, max: u16) -> LinkResult<EventBatch> {
        match self.request(&Request::Events { after, kinds, max })? {
            Response::Events(b) => Ok(b),
            other => Err(unexpected(other)),
        }
    }

    /// Breakpoints, and whether and where the game stands.
    pub fn debug_state(&mut self) -> LinkResult<DebugState> {
        match self.request(&Request::Debug)? {
            Response::Debug(d) => Ok(d),
            other => Err(unexpected(other)),
        }
    }

    /// Sets every breakpoint (an empty list clears them); a stop lasts at
    /// most `hold_s` seconds (0: the module's default).
    pub fn set_breaks(&mut self, rules: Vec<BreakRule>, hold_s: u16) -> LinkResult<Done> {
        match self.request(&Request::Break { rules, hold_s })? {
            Response::Done(d) => Ok(d),
            other => Err(unexpected(other)),
        }
    }

    /// Pause, go on, one frame.
    pub fn control(&mut self, control: Control) -> LinkResult<Done> {
        match self.request(&Request::Control(control))? {
            Response::Done(d) => Ok(d),
            other => Err(unexpected(other)),
        }
    }

    /// The free camera: the game draws with `camera` until `None` (a write).
    pub fn set_camera(&mut self, camera: Option<CameraView>) -> LinkResult<Done> {
        match self.request(&Request::SetCamera { camera })? {
            Response::Done(d) => Ok(d),
            other => Err(unexpected(other)),
        }
    }

    /// New pixels for a loaded texture: the whole `tg4d` payload of the
    /// texture named `name` (a write).
    pub fn texture(&mut self, name: u32, data: Vec<u8>) -> LinkResult<Done> {
        match self.request(&Request::Texture { name, data })? {
            Response::Done(d) => Ok(d),
            other => Err(unexpected(other)),
        }
    }

    /// Whether the module answers the requests of the world and the state
    /// of the game (protocol 4, stage 48): time, weather, streams, money,
    /// family, missions, variables.
    pub fn has_state(&self) -> bool {
        self.version >= 4
    }

    /// Whether the module answers [`Request::Files`] (protocol 5, stage
    /// 46).
    pub fn has_files(&self) -> bool {
        self.version >= 5
    }

    /// The files the game opens from elsewhere and how often it opened
    /// them (stage 46).
    pub fn files(&mut self) -> LinkResult<proto::Files> {
        match self.request(&Request::Files)? {
            Response::Files(f) => Ok(f),
            other => Err(unexpected(other)),
        }
    }

    /// Whether the module composes streams from resource patches and
    /// answers [`Request::Resources`] (protocol 9, stage 100).
    pub fn has_resources(&self) -> bool {
        self.version >= 9
    }

    /// The streams the module composes in memory from the game's file and
    /// a resource patch, and their use (stage 100).
    pub fn resources(&mut self) -> LinkResult<proto::Resources> {
        match self.request(&Request::Resources)? {
            Response::Resources(r) => Ok(r),
            other => Err(unexpected(other)),
        }
    }

    /// The record of the loaded resource with these ids in the game's
    /// table of resources: where its data lie in memory (stage 100).
    pub fn loaded_resource(&mut self, ids: [u32; 4]) -> LinkResult<Option<proto::Loaded>> {
        match self.request(&Request::Resource { ids })? {
            Response::Resource(l) => Ok(l),
            other => Err(unexpected(other)),
        }
    }

    /// The game's clock (`None`: no level).
    pub fn time_of_day(&mut self) -> LinkResult<Option<TimeOfDay>> {
        match self.request(&Request::TimeOfDay)? {
            Response::TimeOfDay(t) => Ok(t),
            other => Err(unexpected(other)),
        }
    }

    /// Sets the time of day and its rate (a write).
    pub fn set_time(&mut self, change: TimeChange) -> LinkResult<Done> {
        self.done(&Request::SetTime(change))
    }

    /// Rain and the clock that decides the sky.
    pub fn weather(&mut self) -> LinkResult<Weather> {
        match self.request(&Request::Weather)? {
            Response::Weather(w) => Ok(w),
            other => Err(unexpected(other)),
        }
    }

    /// Asks the game for a stream (a write); one request at a time.
    pub fn stream_load(&mut self, name: &str) -> LinkResult<Done> {
        self.done(&Request::StreamLoad {
            name: name.to_owned(),
        })
    }

    /// Unloads a stream the link loaded (any with `force`; a write).
    pub fn stream_unload(&mut self, name: &str, force: bool) -> LinkResult<Done> {
        self.done(&Request::StreamUnload {
            name: name.to_owned(),
            force,
        })
    }

    /// The stream requests of the link and whether they are done.
    pub fn stream_queue(&mut self) -> LinkResult<StreamQueue> {
        match self.request(&Request::StreamQueue)? {
            Response::StreamQueue(q) => Ok(q),
            other => Err(unexpected(other)),
        }
    }

    /// The player's money (`None`: no family of the player now).
    pub fn money(&mut self) -> LinkResult<Option<Money>> {
        match self.request(&Request::Money)? {
            Response::Money(m) => Ok(m),
            other => Err(unexpected(other)),
        }
    }

    /// Adds to the player's money or sets it (a write).
    pub fn set_money(&mut self, change: MoneyChange) -> LinkResult<Done> {
        self.done(&Request::SetMoney(change))
    }

    /// The families and the player's crew.
    pub fn family(&mut self) -> LinkResult<Family> {
        match self.request(&Request::Family)? {
            Response::Family(f) => Ok(f),
            other => Err(unexpected(other)),
        }
    }

    /// The current checkpoint and those of the loaded streams.
    pub fn mission(&mut self) -> LinkResult<Mission> {
        match self.request(&Request::Mission)? {
            Response::Mission(m) => Ok(m),
            other => Err(unexpected(other)),
        }
    }

    /// Variables of the story named by `hashes` (of the name,
    /// [`doctor_toolkit_live::name_hash`], or of the record); all when
    /// empty.
    pub fn variables(&mut self, hashes: Vec<u32>) -> LinkResult<Vec<Variable>> {
        match self.request(&Request::Variables { hashes })? {
            Response::Variables(v) => Ok(v),
            other => Err(unexpected(other)),
        }
    }

    /// Sets a variable of the story (a write).
    pub fn set_variable(&mut self, hash: u32, value: i32) -> LinkResult<Done> {
        self.done(&Request::SetVariable { hash, value })
    }

    fn done(&mut self, request: &Request) -> LinkResult<Done> {
        match self.request(request)? {
            Response::Done(d) => Ok(d),
            other => Err(unexpected(other)),
        }
    }

    /// Whether the module edits the world (protocol 6, stage 47): objects
    /// moved, turned, hidden and shown, with undo.
    pub fn has_edits(&self) -> bool {
        self.version >= 6
    }

    /// The state of one object (stage 47).
    pub fn object(&mut self, guid: proto::Guid) -> LinkResult<proto::ObjectState> {
        match self.request(&Request::Object { guid })? {
            Response::Object(s) => Ok(s),
            other => Err(unexpected(other)),
        }
    }

    /// Gives an object a new matrix (a write).
    pub fn object_transform(
        &mut self,
        guid: proto::Guid,
        matrix: proto::Matrix,
    ) -> LinkResult<proto::ObjectEdit> {
        match self.request(&Request::ObjectTransform { guid, matrix })? {
            Response::ObjectEdit(e) => Ok(e),
            other => Err(unexpected(other)),
        }
    }

    /// Hides or shows an object, or stops and starts its logic (a write).
    pub fn object_enable(
        &mut self,
        guid: proto::Guid,
        enabled: bool,
        logic: bool,
    ) -> LinkResult<proto::ObjectEdit> {
        match self.request(&Request::ObjectEnable {
            guid,
            enabled,
            logic,
        })? {
            Response::ObjectEdit(e) => Ok(e),
            other => Err(unexpected(other)),
        }
    }

    /// Undoes the latest edit of the link (of `guid`) in the game (a write).
    pub fn object_undo(&mut self, guid: Option<proto::Guid>) -> LinkResult<proto::ObjectEdit> {
        match self.request(&Request::ObjectUndo { guid })? {
            Response::ObjectEdit(e) => Ok(e),
            other => Err(unexpected(other)),
        }
    }

    /// The edits of the link not undone, oldest first.
    pub fn object_edits(&mut self) -> LinkResult<Vec<proto::ObjectEdit>> {
        match self.request(&Request::ObjectEdits)? {
            Response::ObjectEdits(list) => Ok(list),
            other => Err(unexpected(other)),
        }
    }

    /// Drops edits from the module's journal without undoing them.
    pub fn object_forget(&mut self, guid: Option<proto::Guid>) -> LinkResult<Done> {
        match self.request(&Request::ObjectForget { guid })? {
            Response::Done(d) => Ok(d),
            other => Err(unexpected(other)),
        }
    }

    /// Whether the module says where a message comes from and lists the
    /// deliveries to one listener (protocol 8, stage 78).
    pub fn has_message_sender(&self) -> bool {
        self.version >= 8
    }

    /// Whether the module measures the game (protocol 7, stage 58).
    pub fn has_profile(&self) -> bool {
        self.version >= 7
    }

    /// The profile of the game (stage 58): `reset` empties the module's
    /// rings and counters of reads afterwards, `memory` walks the address
    /// space (up to 2 s), `sample_ms` samples where the main and render
    /// threads stand first (at most 10 s; the answer waits for it).
    pub fn profile(
        &mut self,
        reset: bool,
        memory: bool,
        sample_ms: u32,
    ) -> LinkResult<proto::Profile> {
        let sample_ms = sample_ms.min(proto::profile::MAX_SAMPLE_MS);
        // The answer comes after the sampler and the walk of the memory:
        // wait that much longer for it.
        let before = self.stream.read_timeout().ok().flatten();
        if let Some(t) = before {
            let extra = u64::from(sample_ms) + if memory { 2_500 } else { 0 };
            let _ = self
                .stream
                .set_read_timeout(Some(t + Duration::from_millis(extra)));
        }
        let answer = self.request(&Request::Profile {
            reset,
            memory,
            sample_ms,
        });
        if before.is_some() {
            let _ = self.stream.set_read_timeout(before);
        }
        match answer? {
            Response::Profile(p) => Ok(*p),
            other => Err(unexpected(other)),
        }
    }

    /// Every SimGroup object that passes `filter`, logic included.
    pub fn world(&mut self, filter: WorldFilter, max: u32) -> LinkResult<World> {
        match self.request(&Request::World { filter, max })? {
            Response::World(w) => Ok(w),
            other => Err(unexpected(other)),
        }
    }
}

fn unexpected(r: Response) -> LinkError {
    LinkError::Unexpected(format!("{r:?}"))
}

/// The module file to install: `DOCTOR_LIVE_MODULE`, `doctor_live.asi`
/// next to the running program, or the build of this repository
/// (`tools/build-live-module.sh` → `target/live/doctor_live.asi`).
pub fn module_binary() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("DOCTOR_LIVE_MODULE") {
        let p = PathBuf::from(p);
        return p.is_file().then_some(p);
    }
    let mut candidates = Vec::new();
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        candidates.push(dir.join(MODULE_FILE));
        candidates.push(dir.join("../share/doctor_toolkit").join(MODULE_FILE));
        candidates.push(dir.join("../live").join(MODULE_FILE));
    }
    candidates.push(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/live")
            .join(MODULE_FILE),
    );
    candidates.into_iter().find(|p| p.is_file())
}

/// Where the toolkit records what it installed.
const STATE_DIR: &str = "_doctor_toolkit/live";
/// Copy of the installed module and configuration (to tell them from the
/// player's own files when taking them away).
const WRITTEN_MODULE: &str = "doctor_live.asi.written";
const WRITTEN_CONFIG: &str = "doctor_live.cfg.written";
/// Marks that `scripts/` was created by the toolkit.
const MADE_SCRIPTS: &str = "scripts.made";
/// The module's log of the last launch, kept after the files are taken
/// away.
pub const LAST_LOG: &str = "_doctor_toolkit/live/last.log";

/// The module installed in a game folder for one launch: `scripts/
/// doctor_live.asi` and `scripts/doctor_live.cfg`. [`ModuleInstall::
/// restore`] (or [`restore_stale`] after a crash of the toolkit) takes them
/// away again: files someone changed in between stay, the module's log is
/// moved to [`LAST_LOG`], and `scripts/` goes when the toolkit made it and
/// it is empty.
#[derive(Debug)]
pub struct ModuleInstall {
    root: PathBuf,
    config: Config,
    restored: bool,
}

impl ModuleInstall {
    /// Installs `module` (the DLL bytes) with `config` into `root`.
    pub fn write(root: &Path, module: &[u8], config: &Config) -> Result<Self> {
        restore_stale(root)?;
        let scripts = root.join(SCRIPTS_DIR);
        let state = root.join(STATE_DIR);
        std::fs::create_dir_all(&state).map_err(|e| Error::io(&state, e))?;
        let target = scripts.join(MODULE_FILE);
        if target.exists() {
            return Err(Error::InvalidArgument(format!(
                "{} exists and was not installed by the toolkit",
                target.display()
            )));
        }
        if !scripts.is_dir() {
            std::fs::create_dir_all(&scripts).map_err(|e| Error::io(&scripts, e))?;
            crate::atomic::write(&state.join(MADE_SCRIPTS), b"")?;
        }
        let text = config.to_text();
        crate::atomic::write(&state.join(WRITTEN_MODULE), module)?;
        crate::atomic::write(&state.join(WRITTEN_CONFIG), text.as_bytes())?;
        crate::atomic::write(&target, module)?;
        crate::atomic::write(&scripts.join(CONFIG_FILE), text.as_bytes())?;
        let _ = std::fs::remove_file(scripts.join(LOG_FILE));
        Ok(Self {
            root: root.to_path_buf(),
            config: config.clone(),
            restored: false,
        })
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    pub fn endpoint(&self) -> Endpoint {
        Endpoint {
            port: self.config.port,
            token: self.config.token.clone(),
        }
    }

    /// Takes the files away; returns whether the game folder changed.
    pub fn restore(mut self) -> Result<bool> {
        self.restored = true;
        restore_stale(&self.root)
    }
}

impl Drop for ModuleInstall {
    fn drop(&mut self) {
        if !self.restored {
            let _ = restore_stale(&self.root);
        }
    }
}

/// A start straight into a level (`-stream`, stage 32) leaves the player
/// without control: the game makes him before it knows the controller, so
/// he reads port 0 while the keyboard is port 15 (`docs/analysis/
/// live-link.md` §5.1). The module fixes it ([`Config::fix_input`]). What
/// "Play" does about it when the live link is not switched on anyway.
#[derive(Debug)]
pub enum InputFix {
    /// The module is installed for this launch, writes off: the player moves.
    Installed(ModuleInstall),
    /// No ASI loader in the game folder: the module cannot load and the
    /// player will not move.
    NoLoader,
    /// The module is not built (`tools/build-live-module.sh`).
    NoModule,
}

/// Installs the module for the player's input of a start straight into a
/// level, when the live link is off. With the link on its own installation
/// already fixes the input.
pub fn install_for_input(root: &Path) -> Result<InputFix> {
    if loader::find(root).is_none() {
        return Ok(InputFix::NoLoader);
    }
    let Some(path) = module_binary() else {
        return Ok(InputFix::NoModule);
    };
    let bytes = std::fs::read(&path).map_err(|e| Error::io(&path, e))?;
    let config = Config {
        port: free_port(),
        token: new_token(),
        writes: false,
        fix_input: true,
        // Only the input: no hooks of the journal for a launch without the
        // live link.
        journal: false,
        ..Default::default()
    };
    Ok(InputFix::Installed(ModuleInstall::write(
        root, &bytes, &config,
    )?))
}

/// Whether the toolkit's module is installed in `root` now.
pub fn installed(root: &Path) -> bool {
    root.join(STATE_DIR).join(WRITTEN_MODULE).is_file()
        && root.join(SCRIPTS_DIR).join(MODULE_FILE).is_file()
}

/// Takes away a module the toolkit installed for an earlier launch (see
/// [`ModuleInstall`]). Returns whether the game folder changed.
pub fn restore_stale(root: &Path) -> Result<bool> {
    let state = root.join(STATE_DIR);
    let written_module = state.join(WRITTEN_MODULE);
    let written_config = state.join(WRITTEN_CONFIG);
    if !written_module.exists() && !written_config.exists() {
        return Ok(false);
    }
    let scripts = root.join(SCRIPTS_DIR);
    let mut changed = false;
    for (target, written) in [
        (scripts.join(MODULE_FILE), &written_module),
        (scripts.join(CONFIG_FILE), &written_config),
    ] {
        let mine = std::fs::read(written).ok();
        if mine.is_some() && std::fs::read(&target).ok() == mine {
            std::fs::remove_file(&target).map_err(|e| Error::io(&target, e))?;
            changed = true;
        }
    }
    let log = scripts.join(LOG_FILE);
    if log.is_file() {
        let keep = root.join(LAST_LOG);
        let _ = std::fs::remove_file(&keep);
        std::fs::rename(&log, &keep).map_err(|e| Error::io(&log, e))?;
        changed = true;
    }
    if state.join(MADE_SCRIPTS).exists() {
        // Only when empty: anything else in it is not ours.
        if std::fs::remove_dir(&scripts).is_ok() {
            changed = true;
        }
        let _ = std::fs::remove_file(state.join(MADE_SCRIPTS));
    }
    for path in [&written_module, &written_config] {
        match std::fs::remove_file(path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(Error::io(path, e)),
        }
    }
    Ok(changed)
}

/// How many redirected files the game opened, by the module's log (it logs
/// the first open of each: `files: CreateFileW gf2/cuba.str -> …`).
pub fn redirected_in_log(log: &str) -> usize {
    log.lines()
        .filter(|l| l.contains("] files: ") && l.contains(" -> "))
        .count()
}

/// How many streams the module composed in memory from a resource patch
/// and how many it could not compose (the game read its own file, without
/// the changes), by the module's log (stage 100): `resources: gf2/cuba.str
/// composed in …`, `resources: gf2/cuba.str is not composed, …`. A stream
/// opened again with the same file is composed once.
pub fn composed_in_log(log: &str) -> (usize, usize) {
    let lines = || log.lines().filter(|l| l.contains("] resources: "));
    (
        lines().filter(|l| l.contains(" composed in ")).count(),
        lines().filter(|l| l.contains(" is not composed")).count(),
    )
}

/// A random token for the configuration (from the clock and the process,
/// hashed: enough to keep other local programs from driving the game by
/// accident; the port is on 127.0.0.1 only).
pub fn new_token() -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut out = String::new();
    for i in 0..2u64 {
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u64(i);
        h.write_u128(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default(),
        );
        h.write_u32(std::process::id());
        out += &format!("{:016x}", h.finish());
    }
    out
}

/// A free port on 127.0.0.1 near the default (the module binds the one in
/// its configuration).
pub fn free_port() -> u16 {
    for port in proto::DEFAULT_PORT..proto::DEFAULT_PORT + 64 {
        if std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, port)).is_ok() {
            return port;
        }
    }
    proto::DEFAULT_PORT
}

/// A texture of the workspace that can go to the running game
/// ([`Client::texture`]): its name hash (the last id of its `tg4h`/`tg4d`),
/// stream and the edited `tg4d` payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HotTexture {
    pub name: u32,
    pub stream: String,
    pub data: Vec<u8>,
}

/// Edited textures of a workspace (their `tg4d` changed), optionally only
/// the one named `name` — what can be swapped in the running game without a
/// restart (same size, format and levels as the loaded one; the module
/// checks).
pub fn hot_textures(
    ws: &crate::workspace::Workspace,
    name: Option<u32>,
) -> Result<Vec<HotTexture>> {
    let mut out = Vec::new();
    for change in ws.changes() {
        let key = &change.key;
        if key.type_id != crate::resource::texture::TYPE_TG4D
            || name.is_some_and(|n| n != key.ids[3])
        {
            continue;
        }
        if let Some(data) = ws.data(key)? {
            out.push(HotTexture {
                name: key.ids[3],
                stream: key.stream.clone(),
                data,
            });
        }
    }
    out.sort_by(|a, b| (a.name, &a.stream).cmp(&(b.name, &b.stream)));
    out.dedup_by_key(|t| t.name);
    Ok(out)
}

/// A world matrix as the game keeps it (rows right, up, forward — `+0x40`
/// of a view, `+0xD0` of an entity) for an eye at `position` looking along
/// `forward`, level (no roll): right = up × forward, up = forward × right.
pub fn pose_looking(position: [f32; 3], forward: [f32; 3]) -> Pose {
    let norm = |v: [f32; 3]| {
        let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
        if l > 1e-6 {
            [v[0] / l, v[1] / l, v[2] / l]
        } else {
            [0.0, 0.0, 1.0]
        }
    };
    let cross = |a: [f32; 3], b: [f32; 3]| {
        [
            a[1] * b[2] - a[2] * b[1],
            a[2] * b[0] - a[0] * b[2],
            a[0] * b[1] - a[1] * b[0],
        ]
    };
    let f = norm(forward);
    // Straight up or down: keep a heading along +Z.
    let r = if f[0].abs() < 1e-4 && f[2].abs() < 1e-4 {
        [1.0, 0.0, 0.0]
    } else {
        norm(cross([0.0, 1.0, 0.0], f))
    };
    Pose {
        position,
        right: r,
        up: cross(f, r),
        forward: f,
    }
}

/// The textures of one stream of the game, every level painted one colour
/// (`rgb`) — a check of the texture replacement that is seen at once in the
/// game. 2D textures of known formats only; normal maps are left out.
pub fn tinted_textures(
    game: &crate::game::GameInstance,
    stream: &str,
    rgb: [u8; 3],
) -> Result<Vec<HotTexture>> {
    use crate::resource::texture::pairs;
    use crate::resource::tg4::{Format, Kind, Tg4Header};
    let Some(index) = game.find_stream(stream) else {
        return Ok(Vec::new());
    };
    let file = game.open_stream(index)?;
    let mut out = Vec::new();
    for pair in pairs(file.layout()) {
        let (Ok(head), Ok(mut data)) = (
            file.resource_data(pair.header),
            file.resource_data(pair.data),
        ) else {
            continue;
        };
        let Ok(header) = Tg4Header::parse(&head) else {
            continue;
        };
        if header.kind() != Kind::Flat {
            continue;
        }
        let format = header.format();
        if matches!(format, Format::Dxt5Nm | Format::Other(_)) {
            continue;
        }
        tint(&format, &mut data, rgb);
        out.push(HotTexture {
            name: file.layout().resources[pair.data].header.ids[3],
            stream: stream.to_owned(),
            data,
        });
    }
    Ok(out)
}

/// Paints texture data of `format` one colour, in place (alpha is kept).
pub fn tint(format: &crate::resource::tg4::Format, data: &mut [u8], rgb: [u8; 3]) {
    use crate::resource::tg4::Format;
    let c565 =
        (u16::from(rgb[0] >> 3) << 11) | (u16::from(rgb[1] >> 2) << 5) | u16::from(rgb[2] >> 3);
    let colour_block = |b: &mut [u8]| {
        b[0..2].copy_from_slice(&c565.to_le_bytes());
        b[2..4].copy_from_slice(&c565.to_le_bytes());
        b[4..8].fill(0);
    };
    match format {
        Format::Dxt1c | Format::Dxt1a => data.chunks_exact_mut(8).for_each(colour_block),
        Format::Dxt5 | Format::Dxt5Nm => data
            .chunks_exact_mut(16)
            .for_each(|b| colour_block(&mut b[8..16])),
        Format::Rgba8 => data.chunks_exact_mut(4).for_each(|p| {
            // D3DFMT_A8R8G8B8 in memory: B, G, R, A.
            p[0] = rgb[2];
            p[1] = rgb[1];
            p[2] = rgb[0];
        }),
        Format::L8 | Format::L8A8 | Format::A8 | Format::Other(_) => {}
    }
}

#[cfg(test)]
mod pose_tests {
    use super::*;

    #[test]
    fn poses_look_where_asked() {
        let p = pose_looking([1.0, 2.0, 3.0], [0.0, 0.0, 2.0]);
        assert_eq!(
            (p.right, p.up, p.forward),
            ([1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0])
        );
        // Turned 90° to +X and looking down a little: still a world matrix.
        let p = pose_looking([0.0; 3], [1.0, -0.5, 0.0]);
        let m = [
            p.right[0],
            p.right[1],
            p.right[2],
            0.0,
            p.up[0],
            p.up[1],
            p.up[2],
            0.0,
            p.forward[0],
            p.forward[1],
            p.forward[2],
            0.0,
            0.0,
            0.0,
            0.0,
            1.0,
        ];
        assert!(proto::is_world_matrix(&m, false));
        assert!((p.yaw() - std::f32::consts::FRAC_PI_2).abs() < 1e-5);
        assert!(p.up[1] > 0.0);
        let down = pose_looking([0.0; 3], [0.0, -1.0, 0.0]);
        assert!(down.right == [1.0, 0.0, 0.0]);
    }
}

#[cfg(test)]
mod tests;

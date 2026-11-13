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
//!   trying the editor without the game.
//!
//! See `docs/analysis/live-link.md`.

use std::io::Write as _;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, TcpStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

pub use doctor_toolkit_live as proto;
pub use doctor_toolkit_live::config::Config;
use doctor_toolkit_live::{
    CONFIG_FILE, CameraView, Done, ErrorCode, Failure, Hello, LOG_FILE, LuaReply, MODULE_FILE,
    MessageId, ObjectInfo, PROTOCOL_VERSION, Pose, Request, Response, Status, StreamInfo,
    read_frame,
};

use crate::{Error, Result};

pub mod fake;
pub mod loader;

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
}

impl Client {
    /// Connects and says Hello. `timeout` bounds the connect and every
    /// answer (requests that wait for the game's main loop can take a
    /// while at 1–2 frames a second).
    pub fn connect(endpoint: &Endpoint, client: &str, timeout: Duration) -> LinkResult<Self> {
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
                protocol: PROTOCOL_VERSION,
                module: String::new(),
                edition_ok: false,
                edition: String::new(),
                writes: false,
            },
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

    /// Changes how long answers may take.
    pub fn set_timeout(&self, timeout: Duration) {
        let _ = self.stream.set_read_timeout(Some(timeout));
    }

    /// Sends one request and reads its answer; an error answer becomes
    /// [`LinkError::Refused`].
    pub fn request(&mut self, request: &Request) -> LinkResult<Response> {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        self.stream
            .write_all(&request.to_frame(id))
            .map_err(LinkError::Io)?;
        let (header, payload) = read_frame(&mut self.stream).map_err(LinkError::Io)?;
        if header.version != PROTOCOL_VERSION {
            return Err(LinkError::Refused(Failure {
                code: ErrorCode::Version,
                message: format!(
                    "the module speaks protocol {}, the toolkit {PROTOCOL_VERSION}",
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

#[cfg(test)]
mod tests;

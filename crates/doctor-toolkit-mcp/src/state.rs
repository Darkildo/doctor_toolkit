//! The game the server reads and what it builds from it once: the game
//! index, the resource table, the name dictionary, the message index, the
//! dialogue catalog. The workspace and the project are opened again on
//! every call (read-only for the reading tools), so edits made meanwhile in
//! the editor are seen. With `--allow-edits` the writing tools record edits
//! in the workspace under its lock ([`Toolkit::writer`]); with
//! `--allow-game` the server may write the workspace into the game and run
//! it ([`Toolkit::session`]).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::SystemTime;

use doctor_toolkit::dialogue::Catalog;
use doctor_toolkit::game::{GameInstance, ResourceIndex};
use doctor_toolkit::level::Location;
use doctor_toolkit::live::Endpoint;
use doctor_toolkit::logic_graph::{MessageIndex, PinKind};
use doctor_toolkit::navigation::index::default_cache_file;
use doctor_toolkit::navigation::{Dictionary, GameIndex, OpenReport};
use doctor_toolkit::play::LaunchPlan;
use doctor_toolkit::project::Project;
use doctor_toolkit::resource::simgroup::Guid;
use doctor_toolkit::workspace::{Workspace, WorkspaceLock};

use crate::game::{GameSession, XServer};
use crate::output::DEFAULT_MAX_CHARS;

/// What to serve.
#[derive(Debug, Clone)]
pub struct Config {
    /// The game folder.
    pub game: PathBuf,
    /// Workspace folder [default: from the project, else `<game>/_doctor_toolkit/workspace`].
    pub workspace: Option<PathBuf>,
    /// A project folder (`docs/project.md`), for `project_status`.
    pub project: Option<PathBuf>,
    /// The cache file of the game index; `None` builds the index in memory.
    pub index_cache: Option<PathBuf>,
    /// Limit of one tool result in characters.
    pub max_chars: usize,
    /// Tools that record edits in the workspace (`--allow-edits`).
    pub allow_edits: bool,
    /// Tools that write into the game files, start and stop the game and
    /// write into the running game over the live link (`--allow-game`).
    pub allow_game: bool,
    /// The live link to talk to instead of the one the game folder names
    /// (tests: the fake module).
    pub live: Option<Endpoint>,
    /// How `play` starts the game instead of the launch method found for it
    /// (tests: a program standing in for the game).
    pub launch: Option<LaunchPlan>,
    /// The X server `play` runs the game on [default: Xvfb].
    pub x_server: Option<XServer>,
    /// The editor's bridge socket [default: `doctor_toolkit::bridge::socket_path`].
    pub bridge: Option<PathBuf>,
    /// The folder export tools write files into (`--output-dir`); without
    /// it they refuse.
    pub output_dir: Option<PathBuf>,
}

impl Config {
    /// Serves `game` with the default cache of the game index (as the CLI).
    pub fn new(game: impl Into<PathBuf>) -> Self {
        let game = game.into();
        Self {
            index_cache: default_cache_file(&game),
            game,
            workspace: None,
            project: None,
            max_chars: DEFAULT_MAX_CHARS,
            allow_edits: false,
            allow_game: false,
            live: None,
            launch: None,
            x_server: None,
            bridge: doctor_toolkit::bridge::socket_path(),
            output_dir: None,
        }
    }
}

/// A tool error the model reads: what went wrong and what to do.
#[derive(Debug, Clone)]
pub struct ToolError(pub String);

impl<E: std::fmt::Display> From<E> for ToolError {
    fn from(e: E) -> Self {
        Self(e.to_string())
    }
}

pub type ToolResult<T> = std::result::Result<T, ToolError>;

/// Shorthand for a tool error.
pub fn fail<T>(text: impl Into<String>) -> ToolResult<T> {
    Err(ToolError(text.into()))
}

/// An object of a SimGroup: the group and the object's position in it.
pub type ObjectKey = (Location, usize);

/// A pin of an object with a message: message hash, kind, pin name.
pub type MessagePin = (u32, PinKind, String);

/// Who sends and hears what, by message and by object.
pub struct Messages {
    pub index: MessageIndex,
    /// Pins with messages of every object: `(group, object)` → `(message
    /// hash, kind, pin name)`.
    pub by_object: HashMap<ObjectKey, Vec<MessagePin>>,
}

impl Messages {
    fn build(game: &GameInstance, resources: &ResourceIndex, ws: Option<&Workspace>) -> Self {
        let index = MessageIndex::build(game, resources, ws);
        let mut by_object: HashMap<ObjectKey, Vec<MessagePin>> = HashMap::new();
        for (&hash, sites) in &index.by_hash {
            for s in sites.senders.iter().chain(&sites.listeners) {
                by_object.entry((s.group, s.object)).or_default().push((
                    hash,
                    s.kind,
                    s.pin.clone(),
                ));
            }
        }
        for pins in by_object.values_mut() {
            pins.sort_by(|a, b| a.2.cmp(&b.2).then(a.0.cmp(&b.0)));
            pins.dedup();
        }
        Self { index, by_object }
    }
}

/// When the workspace was last saved: caches built with its data are
/// rebuilt when it changes.
type WsStamp = Option<(SystemTime, u64)>;

/// The game being served and what is built from it on first use.
pub struct Toolkit {
    pub game: Arc<GameInstance>,
    /// The workspace folder (it may not exist).
    pub workspace_dir: PathBuf,
    pub project_dir: Option<PathBuf>,
    pub max_chars: usize,
    pub allow_edits: bool,
    pub allow_game: bool,
    pub live: Option<Endpoint>,
    pub launch: Option<LaunchPlan>,
    pub x_server: Option<XServer>,
    pub bridge: Option<PathBuf>,
    pub output_dir: Option<PathBuf>,
    /// Held by a writing tool for its whole call: one write at a time.
    writer: Mutex<()>,
    /// The game started by `play`.
    pub session: Mutex<Option<GameSession>>,
    /// GUIDs of every SimGroup object of the game files.
    guids: OnceLock<std::collections::HashSet<Guid>>,
    cache: Option<PathBuf>,
    index: OnceLock<(GameIndex, OpenReport)>,
    resources: OnceLock<ResourceIndex>,
    dictionary: OnceLock<Dictionary>,
    messages: Mutex<Option<(WsStamp, Arc<Messages>)>>,
    dialogues: OnceLock<Result<Catalog, String>>,
    /// Names of messages and objects of the game files, for the journal of
    /// the running game.
    live_names: OnceLock<doctor_toolkit::live::names::LiveNames>,
}

impl Toolkit {
    /// Opens the game of `config` (scans its folder; nothing is read yet).
    pub fn open(config: &Config) -> doctor_toolkit::Result<Self> {
        let game = GameInstance::open(&config.game)?;
        let project_dir = config.project.clone();
        let workspace_dir = match (&config.workspace, &project_dir) {
            (Some(dir), _) => dir.clone(),
            (None, Some(p)) => Project::open(p)
                .ok()
                .and_then(|p| p.config.workspace_dir(p.root()))
                .unwrap_or_else(|| Workspace::default_dir(game.root())),
            (None, None) => Workspace::default_dir(game.root()),
        };
        Ok(Self {
            game: Arc::new(game),
            workspace_dir,
            project_dir,
            max_chars: config.max_chars.max(crate::output::MIN_MAX_CHARS),
            allow_edits: config.allow_edits,
            allow_game: config.allow_game,
            live: config.live.clone(),
            launch: config.launch.clone(),
            x_server: config.x_server.clone(),
            bridge: config.bridge.clone(),
            output_dir: config.output_dir.clone(),
            writer: Mutex::new(()),
            session: Mutex::new(None),
            guids: OnceLock::new(),
            cache: config.index_cache.clone(),
            index: OnceLock::new(),
            resources: OnceLock::new(),
            dictionary: OnceLock::new(),
            messages: Mutex::new(None),
            dialogues: OnceLock::new(),
            live_names: OnceLock::new(),
        })
    }

    /// The game index (names and references), loaded from its cache or built.
    pub fn index(&self) -> &GameIndex {
        &self.index_with_report().0
    }

    pub fn index_with_report(&self) -> &(GameIndex, OpenReport) {
        self.index
            .get_or_init(|| GameIndex::open(&self.game, self.cache.as_deref(), &|_, _| {}))
    }

    /// The cache file of the game index, when there is one.
    pub fn index_cache(&self) -> Option<&Path> {
        self.cache.as_deref()
    }

    /// Headers of every resource by stream (what the logic graph and the
    /// level code take).
    pub fn resources(&self) -> &ResourceIndex {
        self.resources
            .get_or_init(|| self.game.build_index(|_| {}).0)
    }

    /// Names of hashes: the built-in lists and every name of the game.
    pub fn dictionary(&self) -> &Dictionary {
        self.dictionary
            .get_or_init(|| Dictionary::for_game(self.index()))
    }

    /// The workspace, opened read-only, when it exists.
    pub fn workspace(&self) -> ToolResult<Option<Workspace>> {
        if !Workspace::exists(&self.workspace_dir) {
            return Ok(None);
        }
        Workspace::open_read_only(&self.workspace_dir)
            .map(Some)
            .map_err(|e| ToolError(format!("cannot read the workspace: {e}")))
    }

    /// The workspace opened for writing: the server's own write lock, the
    /// workspace's lock (refused, naming the holder, when the editor or a
    /// command writes it) and the workspace read under it (made in memory
    /// when there is none; the first recorded edit writes it).
    ///
    /// When the editor holds the workspace and listens on the bridge
    /// (`doctor_toolkit::bridge`), the user is asked there to allow the
    /// write (`what` says what it is); allowed, the editor lends its
    /// workspace for this one call and reads it again afterwards.
    pub fn write_workspace(&self, what: &str) -> ToolResult<WriteAccess<'_>> {
        let guard = self.writer.lock().unwrap_or_else(|e| e.into_inner());
        let (lock, bridge) = match WorkspaceLock::acquire(&self.workspace_dir, LOCK_HOLDER) {
            Ok(lock) => (lock, None),
            Err(e) => {
                let held_by_editor = WorkspaceLock::holder(&self.workspace_dir)
                    .is_some_and(|h| h.program == crate::bridge::EDITOR);
                match held_by_editor.then(|| crate::bridge::lend(self, what)) {
                    Some(Ok(client)) => {
                        let lock = WorkspaceLock::acquire(&self.workspace_dir, LOCK_HOLDER)
                            .map_err(|e| ToolError(format!("{e} (after the editor allowed it)")))?;
                        (lock, Some(client))
                    }
                    Some(Err(why)) => {
                        return fail(format!(
                            "{e}; {}. The workspace can be written by one program at a time",
                            why.0
                        ));
                    }
                    None => {
                        return fail(format!(
                            "{e}. The workspace can be written by one program at a time: while \
                             the editor has it open, edit there (or close the game in the \
                             editor); a command line command ends by itself"
                        ));
                    }
                }
            }
        };
        let ws = Workspace::for_game(&self.game, Some(&self.workspace_dir))
            .map_err(|e| ToolError(format!("cannot open the workspace: {e}")))?;
        Ok(WriteAccess {
            ws,
            lock: Some(lock),
            bridge,
            _guard: guard,
        })
    }

    /// GUIDs of every SimGroup object of the game files, so that new
    /// objects get GUIDs of their own (read once).
    pub fn object_guids(&self) -> &std::collections::HashSet<Guid> {
        self.guids.get_or_init(|| {
            let simgroup = doctor_toolkit::level::TYPE_SIMGROUP;
            let mut by_stream: HashMap<usize, Vec<usize>> = HashMap::new();
            for e in self
                .resources()
                .entries
                .iter()
                .filter(|e| e.header.type_id == simgroup)
            {
                by_stream
                    .entry(e.stream as usize)
                    .or_default()
                    .push(e.resource as usize);
            }
            let mut out = std::collections::HashSet::new();
            for (s, resources) in by_stream {
                let Ok(stream) = self.game.open_stream(s) else {
                    continue;
                };
                for r in resources {
                    if let Some(g) = stream
                        .resource_data(r)
                        .ok()
                        .and_then(|d| doctor_toolkit::resource::simgroup::SimGroup::parse(&d).ok())
                    {
                        out.extend(g.objects.into_iter().map(|o| o.guid));
                    }
                }
            }
            out
        })
    }

    /// Stops the game started by `play` (when the client goes away).
    pub fn shutdown(&self) {
        let session = self
            .session
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        if let Some(session) = session {
            let _ = crate::tools::play::finish(self, session);
        }
    }

    /// The workspace unless `original`.
    pub fn workspace_unless(&self, original: bool) -> ToolResult<Option<Workspace>> {
        if original { Ok(None) } else { self.workspace() }
    }

    fn workspace_stamp(&self) -> WsStamp {
        let meta = std::fs::metadata(self.workspace_dir.join("manifest.json")).ok()?;
        Some((meta.modified().ok()?, meta.len()))
    }

    /// The message index of the game as the workspace has it; rebuilt when
    /// the workspace changed since.
    pub fn messages(&self) -> ToolResult<Arc<Messages>> {
        let stamp = self.workspace_stamp();
        let mut cached = self.messages.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((s, m)) = cached.as_ref()
            && *s == stamp
        {
            return Ok(m.clone());
        }
        let ws = self.workspace()?;
        let built = Arc::new(Messages::build(&self.game, self.resources(), ws.as_ref()));
        *cached = Some((stamp, built.clone()));
        Ok(built)
    }

    /// Names of the messages and objects of the game files (read once): what
    /// the events of the running game's journal are named with.
    pub fn live_names(&self) -> &doctor_toolkit::live::names::LiveNames {
        self.live_names.get_or_init(|| {
            doctor_toolkit::live::names::LiveNames::build(&self.game, self.resources(), None)
        })
    }

    /// The dialogue catalog (read once).
    pub fn dialogues(&self) -> ToolResult<&Catalog> {
        self.dialogues
            .get_or_init(|| Catalog::build(&self.game, &|_, _| {}).map_err(|e| e.to_string()))
            .as_ref()
            .map_err(|e| ToolError(format!("cannot read the dialogues: {e}")))
    }

    /// Where an export tool writes `name` (a relative path such as
    /// `textures/brick.png`): inside the output folder given at start
    /// (`--output-dir`), its folders made. Refused without that folder and
    /// for names that would leave it.
    pub fn output_path(&self, name: &str) -> ToolResult<PathBuf> {
        let Some(dir) = &self.output_dir else {
            return fail(
                "this server has no output folder: start it with --output-dir DIR \
                 (`doctor-toolkit mcp GAME --output-dir DIR`) to export files",
            );
        };
        let rel = Path::new(name.trim());
        let clean = rel
            .components()
            .all(|c| matches!(c, std::path::Component::Normal(_)));
        if name.trim().is_empty() || !clean {
            return fail(format!(
                "`{name}`: an output name is a relative path without `..`"
            ));
        }
        let path = dir.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| ToolError(format!("cannot make {}: {e}", parent.display())))?;
        }
        Ok(path)
    }

    /// The stream called `name`; only streams of the game are found, so no
    /// path outside the game folder can be named.
    pub fn stream(&self, name: &str) -> ToolResult<usize> {
        let name = name.trim();
        if name.contains("..") {
            return fail(format!("`{name}` is not a stream name"));
        }
        self.game
            .find_stream(name.trim_end_matches(".str"))
            .ok_or_else(|| ToolError(format!("no stream `{name}`; `list_streams` lists them")))
    }

    /// Data of resource `resource` of stream `stream`: the workspace's
    /// version when `ws` is given and changes it, else the game file's.
    pub fn data(
        &self,
        ws: Option<&Workspace>,
        stream: usize,
        resource: usize,
    ) -> ToolResult<Vec<u8>> {
        Ok(match ws {
            Some(ws) => ws.current_data(&self.game, stream, resource)?,
            None => self.game.open_stream(stream)?.resource_data(resource)?,
        })
    }
}

/// How the server names itself to programs that find the workspace locked.
pub const LOCK_HOLDER: &str = "the MCP server (doctor-toolkit mcp)";

/// The workspace open for writing; see [`Toolkit::write_workspace`].
pub struct WriteAccess<'a> {
    pub ws: Workspace,
    lock: Option<WorkspaceLock>,
    /// The editor that lent its workspace: it gets it back when the write is done.
    bridge: Option<crate::bridge::Lent>,
    _guard: std::sync::MutexGuard<'a, ()>,
}

impl Drop for WriteAccess<'_> {
    fn drop(&mut self) {
        // The lock first: the editor takes it when it gets the workspace back.
        self.lock.take();
        if let Some(lent) = self.bridge.take() {
            lent.give_back();
        }
    }
}

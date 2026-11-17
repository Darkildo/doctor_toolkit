//! `live`: the live link with the running game (stage 34); the game's
//! journal and every object (stage 49); breakpoints, pause and step, the
//! free camera and textures without a restart (stage 35); the world and
//! the state of the game — time, weather, streams, money, family,
//! missions, variables (stage 48, [`crate::live_state_cmd`]); editing the
//! world while it runs (stage 47, [`crate::live_object_cmd`]).

use std::io::Write as _;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use clap::{Args, Subcommand};
use serde_json::json;

use doctor_toolkit::live::names::LiveNames;
use doctor_toolkit::live::proto::journal::mask;
use doctor_toolkit::live::proto::{self, MessageId, World, WorldFilter, WorldObject};
pub use doctor_toolkit::live::view::object_label;
use doctor_toolkit::live::view::{debug_json, debug_lines, event_json, event_lines, free_camera};
use doctor_toolkit::live::{self, Client, Config, Endpoint, ModuleInstall};

macro_rules! out {
    ($($arg:tt)*) => {
        if writeln!(std::io::stdout(), $($arg)*).is_err() {
            std::process::exit(0);
        }
    };
}

/// Where the module listens.
#[derive(Args, Clone)]
pub struct LinkArgs {
    /// Game folder: port and token from its scripts/doctor_live.cfg
    #[arg(long, value_name = "DIR")]
    game: Option<PathBuf>,
    /// Port on 127.0.0.1 [default: 47801, or that of --game]
    #[arg(long)]
    port: Option<u16>,
    /// Token of the module's configuration
    #[arg(long)]
    token: Option<String>,
    /// Seconds to wait for an answer (the game may draw 1–2 frames a second)
    #[arg(long, default_value_t = 30)]
    pub timeout: u64,
    /// Print JSON
    #[arg(long)]
    pub json: bool,
    /// Names of objects and messages from the SimGroups of the --game folder
    /// and its workspace (reads every SimGroup first: a few seconds)
    #[arg(long, requires = "game")]
    names: bool,
}

impl LinkArgs {
    /// The game folder of `--game`.
    pub fn game_dir(&self) -> Option<&std::path::Path> {
        self.game.as_deref()
    }

    fn endpoint(&self) -> Endpoint {
        let mut e = self
            .game
            .as_deref()
            .map(Endpoint::of_game)
            .unwrap_or_default();
        if let Some(p) = self.port {
            e.port = p;
        }
        if let Some(t) = &self.token {
            e.token = t.clone();
        }
        e
    }

    /// The index of names, when asked for (or `needed`).
    pub fn names(&self, needed: bool) -> Result<Option<LiveNames>> {
        if !(self.names || needed) {
            return Ok(None);
        }
        let dir = self
            .game
            .as_deref()
            .context("names come from the SimGroups of a game folder: add --game DIR")?;
        let names = LiveNames::open(dir, false)
            .with_context(|| format!("reading the SimGroups of {}", dir.display()))?;
        if names.errors > 0 {
            eprintln!("warning: {} SimGroup(s) could not be read", names.errors);
        }
        Ok(Some(names))
    }

    pub fn connect(&self) -> Result<Client> {
        let e = self.endpoint();
        let client = Client::connect(
            &e,
            concat!("doctor-toolkit ", env!("CARGO_PKG_VERSION")),
            Duration::from_secs(self.timeout.max(1)),
        )
        .with_context(|| format!("connecting to the game on 127.0.0.1:{}", e.port))?;
        if !client.hello().edition_ok {
            bail!(
                "the module runs in another build of the game and does nothing: {}",
                client.hello().edition
            );
        }
        Ok(client)
    }
}

#[derive(Subcommand)]
pub enum LiveCommand {
    /// Whether the module answers: edition, main loop state, game time
    Status(LinkArgs),
    /// Position and facing of the player
    Player(LinkArgs),
    /// Position, direction and field of view of the game's camera
    Camera(LinkArgs),
    /// Streams the game has loaded
    Streams(LinkArgs),
    /// Load and unload streams one at a time, list them with the requests
    /// of the link
    #[command(subcommand)]
    Stream(crate::live_state_cmd::StreamCommand),
    /// The time of day: print it, or set it (HH:MM[:SS]) and how fast it
    /// goes (a write: needs --yes); sky, fog and light follow
    Time {
        /// HH:MM or HH:MM:SS
        at: Option<String>,
        /// Game seconds per real second (0 stops the clock)
        #[arg(long, conflicts_with = "default_rate")]
        rate: Option<f32>,
        /// Back to the rate of the level
        #[arg(long)]
        default_rate: bool,
        #[arg(long)]
        yes: bool,
        #[command(flatten)]
        link: LinkArgs,
    },
    /// The weather: rain, and what decides the sky
    Weather(LinkArgs),
    /// Files of the game folder the module gives the game from elsewhere
    /// (`play` without writing into the game, stage 46) and how often the
    /// game opened each
    Files(LinkArgs),
    /// Streams the module composes in memory from the game's files and
    /// resource patches (`play --resources-in-memory`, stage 100) and how
    /// the game used each
    Resources(LinkArgs),
    /// A loaded resource in the game's table of resources, by its ids:
    /// where its data lie in memory; with --out the data are saved, with
    /// --compare they are compared with a file
    Resource {
        /// The ids: one hex number (all four equal) or four joined by dots
        ids: String,
        /// Save the data of the resource as they are in memory
        #[arg(long, value_name = "FILE")]
        out: Option<PathBuf>,
        /// Compare the data in memory with this file byte by byte
        #[arg(long, value_name = "FILE")]
        compare: Option<PathBuf>,
        #[command(flatten)]
        link: LinkArgs,
    },
    /// Compare the resources the game holds in memory with stream files
    /// (stage 100): for every resource of the named streams of EXPECTED —
    /// a folder with the streams as `apply` writes them — the record of
    /// the game's table of resources is found by its ids and its data are
    /// compared byte by byte. With --original only the resources that
    /// differ from that game folder (changed or added) are compared in
    /// full and listed; the others are counted by type from their first
    /// --sample bytes
    ResourceCheck {
        /// Folder with the expected streams (a game folder or a copy with
        /// the workspace or the mods applied)
        expected: PathBuf,
        /// Stream names, e.g. gf2/cuba/local (with or without .str)
        #[arg(required = true)]
        streams: Vec<String>,
        /// The unchanged game: resources equal to its are only sampled
        #[arg(long, value_name = "DIR")]
        original: Option<PathBuf>,
        /// Bytes compared of a resource that is not changed
        #[arg(long, default_value_t = 4096)]
        sample: u32,
        #[command(flatten)]
        link: LinkArgs,
    },
    /// Measure the game from inside (stage 58): main-loop and present
    /// times, CPU of its threads, reads of files, textures, the clock;
    /// with --memory its address space, with --sample where its threads
    /// stand. Read-only
    Profile {
        /// Empty the module's rings and read counters after this answer
        #[arg(long)]
        reset: bool,
        /// Walk the address space too (up to 2 s)
        #[arg(long)]
        memory: bool,
        /// Sample where the main and render threads stand for MS
        /// milliseconds first (at most 10 000)
        #[arg(long, value_name = "MS", default_value_t = 0)]
        sample: u32,
        #[command(flatten)]
        link: LinkArgs,
    },
    /// The player's money: print it, or --set / --add (a write: needs --yes)
    Money {
        #[arg(long, allow_hyphen_values = true, conflicts_with = "add")]
        set: Option<f32>,
        #[arg(long, allow_hyphen_values = true)]
        add: Option<f32>,
        #[arg(long)]
        yes: bool,
        #[command(flatten)]
        link: LinkArgs,
    },
    /// The player's crew (members, ranks, specialties) and the families
    Family(LinkArgs),
    /// The current checkpoint and those of the loaded streams
    Mission(LinkArgs),
    /// Variables of the story (variabledictionary): get and set by name
    #[command(subcommand)]
    Var(crate::live_state_cmd::VarCommand),
    /// Edit the world while the game runs: move, turn, hide and show
    /// SimGroup objects, undo, and write the moves into the workspace
    #[command(subcommand)]
    Object(crate::live_object_cmd::ObjectCommand),
    /// SimGroup objects near the player (or a point): GUID, class, position;
    /// with --all every object of the game, logic included
    Objects {
        #[command(flatten)]
        link: LinkArgs,
        /// Metres around the player or --at
        #[arg(long, default_value_t = 30.0)]
        radius: f32,
        /// Only this class (name, e.g. VariableSwitch, or 0x hash)
        #[arg(long)]
        class: Option<String>,
        /// Centre X,Y,Z instead of the player
        #[arg(long, value_name = "X,Y,Z", allow_hyphen_values = true)]
        at: Option<String>,
        /// At most this many [default: 50, with --all 16000]
        #[arg(long)]
        max: Option<u32>,
        /// Every object of the SimManager: logic, triggers and relays too,
        /// with group (stream) and whether it listens to messages
        #[arg(long)]
        all: bool,
        /// With --all: only objects without a world matrix
        #[arg(long, requires = "all")]
        logic: bool,
        /// With --all: only objects of streams whose name contains this
        #[arg(long, requires = "all")]
        stream: Option<String>,
    },
    /// Find objects of the running game by GUID ({…} or 32 hex digits) or by
    /// name (needs --game: names come from its SimGroups)
    Find {
        /// GUID, or a part of an object name
        query: String,
        #[command(flatten)]
        link: LinkArgs,
    },
    /// The game's journal: what it prints, Lua errors with their call stack,
    /// messages of the bus (--messages)
    Log {
        /// Keep printing new events until Ctrl+C
        #[arg(long, short)]
        follow: bool,
        /// Only lines that contain this (case does not matter)
        #[arg(long)]
        filter: Option<String>,
        /// Also messages of the bus with their receivers (the module records
        /// them while someone asks)
        #[arg(long)]
        messages: bool,
        /// Only Lua errors
        #[arg(long, conflicts_with = "messages")]
        errors: bool,
        /// Start after this event number [default: all the module kept]
        #[arg(long, default_value_t = 0)]
        after: u64,
        #[command(flatten)]
        link: LinkArgs,
    },
    /// Move the player (a write: needs --yes)
    Teleport {
        /// X,Y,Z (Y up)
        #[arg(allow_hyphen_values = true)]
        position: String,
        /// Turn to this heading in degrees (0 = +Z) [default: keep]
        #[arg(long, allow_hyphen_values = true)]
        yaw: Option<f32>,
        /// Confirm the write
        #[arg(long)]
        yes: bool,
        #[command(flatten)]
        link: LinkArgs,
    },
    /// Send a SimGroup message, to every listener or one object (a write: needs --yes)
    Send {
        /// Message name, or 0x hash
        message: String,
        /// Only the object with this GUID ({…} or 32 hex digits)
        #[arg(long, value_name = "GUID")]
        to: Option<String>,
        #[arg(long)]
        yes: bool,
        #[command(flatten)]
        link: LinkArgs,
    },
    /// Run Lua 5.0 code in the game and print what it returns (a write: needs --yes)
    Lua {
        code: String,
        #[arg(long)]
        yes: bool,
        #[command(flatten)]
        link: LinkArgs,
    },
    /// Read bytes of the game's memory (diagnostics)
    Peek {
        /// Address (0x…)
        address: String,
        #[arg(default_value_t = 64)]
        len: u16,
        #[command(flatten)]
        link: LinkArgs,
    },
    /// Put the module into a game folder (a shadow copy, or the game for one
    /// launch); `play --live` does this and takes it away after the game exits
    Install {
        game_dir: PathBuf,
        /// Allow teleport, messages and Lua
        #[arg(long)]
        writes: bool,
        #[arg(long)]
        port: Option<u16>,
        /// The module file [default: built one, see tools/build-live-module.sh]
        #[arg(long)]
        module: Option<PathBuf>,
        /// Give the game the workspace without writing it into the game
        /// folder: its files are staged outside and the module redirects
        /// the game to them (stage 46) until `live uninstall`
        #[arg(long, conflicts_with = "mods")]
        apply: bool,
        /// With --apply: the workspace [default: <GAME_DIR>/_doctor_toolkit/workspace]
        #[arg(long, value_name = "DIR", requires = "apply")]
        workspace_dir: Option<PathBuf>,
        /// Give the game the enabled mods of the mod manager the same way
        #[arg(long)]
        mods: bool,
        /// Give the game this `cmd.txt` through the module (its launch
        /// options, e.g. "-stream gf2 cuba"), not written into the folder
        #[arg(long, value_name = "TEXT")]
        cmd_txt: Option<String>,
        /// Log every file of the game folder the game opens (doctor_live.log)
        #[arg(long)]
        files_log: bool,
        /// With --apply or --mods: put the changed resources into the
        /// game's streams in memory when the game loads them (resource
        /// patches, stage 100) instead of rebuilding the streams
        #[arg(long)]
        resources_in_memory: bool,
        /// With --resources-in-memory: the module writes every stream it
        /// composed next to its patch (`<patch>.composed`), for comparing
        /// it with what `apply` writes
        #[arg(long, requires = "resources_in_memory")]
        resources_dump: bool,
    },
    /// Take the module (and the files staged for it) away from a game folder
    Uninstall { game_dir: PathBuf },
    /// Answer like the module, without the game (to try the editor and scripts)
    Fake {
        #[arg(long, default_value_t = proto::DEFAULT_PORT)]
        port: u16,
        #[arg(long)]
        writes: bool,
        /// Make the game's own traffic every second: a trigger sends
        /// doctor_ping, a traced node of doctor_demo prints, an engine
        /// message goes out sixty times
        #[arg(long)]
        activity: bool,
    },
    /// Breakpoints: stop the game when a message goes through the bus, the
    /// game prints a text (a traced node of a visual script) or Lua fails.
    /// Without a rule: print the breakpoints and whether the game stands
    Break {
        /// A message (name or 0x hash)
        #[arg(long)]
        message: Vec<String>,
        /// With --message: only when this object (GUID) gets it
        #[arg(long, value_name = "GUID")]
        to: Option<String>,
        /// Printed text that contains this (dt:SCRIPT:NODE; for a node)
        #[arg(long)]
        print: Vec<String>,
        /// Any Lua error
        #[arg(long)]
        lua_error: bool,
        /// Each rule goes away after its first hit
        #[arg(long)]
        once: bool,
        /// Keep the breakpoints set before and add these
        #[arg(long)]
        add: bool,
        /// Remove every breakpoint
        #[arg(long, conflicts_with_all = ["message", "print", "lua_error", "add"])]
        clear: bool,
        /// Seconds a stop lasts at most before the game goes on by itself
        /// [default: 120, at most 900]
        #[arg(long, default_value_t = 0)]
        hold: u16,
        /// Wait until the game stops and print where
        #[arg(long)]
        wait: bool,
        #[command(flatten)]
        link: LinkArgs,
    },
    /// Stop the game in its next level frame
    Pause(LinkArgs),
    /// Let a stopped game go on (or one frame with --step)
    Continue {
        #[arg(long)]
        step: bool,
        #[command(flatten)]
        link: LinkArgs,
    },
    /// The free camera: the game draws from this point (a write: needs
    /// --yes); --release gives the game its camera back
    Freecam {
        /// X,Y,Z [default: where the game's camera is]
        #[arg(allow_hyphen_values = true)]
        position: Option<String>,
        /// Heading in degrees (0 = +Z) [default: the camera's]
        #[arg(long, allow_hyphen_values = true)]
        yaw: Option<f32>,
        /// Pitch in degrees, up is positive [default: the camera's]
        #[arg(long, allow_hyphen_values = true)]
        pitch: Option<f32>,
        #[arg(long, conflicts_with_all = ["position", "yaw", "pitch"])]
        release: bool,
        #[arg(long)]
        yes: bool,
        #[command(flatten)]
        link: LinkArgs,
    },
    /// Put the edited textures of the workspace into the running game
    /// without a restart (same size, format and levels; a write: needs
    /// --yes). Needs --game
    Texture {
        /// Only this texture: its name hash (0x…, the last id of its
        /// tg4h/tg4d) [default: every edited texture]
        name: Option<String>,
        /// A check instead of the workspace: every texture of this stream
        /// (e.g. gf2/cuba/local/yuri/high/terrain3), painted --tint
        #[arg(long, requires = "tint")]
        stream: Option<String>,
        /// The colour of --stream, RRGGBB
        #[arg(long, requires = "stream")]
        tint: Option<String>,
        #[arg(long)]
        yes: bool,
        #[command(flatten)]
        link: LinkArgs,
    },
}

impl LiveCommand {
    pub fn may_write_game(&self) -> bool {
        // Texture reads the workspace; it writes the running game only.
        matches!(self, Self::Install { .. } | Self::Uninstall { .. })
    }
}

fn vec3(text: &str) -> Result<[f32; 3]> {
    let parts: Vec<f32> = text
        .split(',')
        .map(|p| p.trim().parse::<f32>())
        .collect::<std::result::Result<_, _>>()
        .with_context(|| format!("`{text}` is not X,Y,Z"))?;
    match parts.as_slice() {
        [x, y, z] => Ok([*x, *y, *z]),
        _ => bail!("`{text}` is not X,Y,Z"),
    }
}

fn class_hash(text: &str) -> u32 {
    match MessageId::parse(text) {
        MessageId::Hash(h) => h,
        MessageId::Name(n) => proto::name_hash(&n),
    }
}

fn pose_json(p: &proto::Pose) -> serde_json::Value {
    json!({
        "position": p.position,
        "forward": p.forward,
        "up": p.up,
        "right": p.right,
        "yaw_degrees": p.yaw().to_degrees(),
    })
}

/// Prints the objects of a World answer.
fn print_world(world: &World, names: Option<&LiveNames>, json_out: bool, stream: Option<&str>) {
    let keep = |o: &WorldObject| {
        stream.is_none_or(|want| {
            world
                .stream_name(o.group)
                .is_some_and(|n| n.to_ascii_lowercase().contains(&want.to_ascii_lowercase()))
        })
    };
    if json_out {
        let v: Vec<_> = world
            .objects
            .iter()
            .filter(|o| keep(o))
            .map(|o| {
                let known = names.and_then(|n| n.object(&o.guid));
                json!({
                    "guid": proto::guid_hex(&o.guid), "braced": proto::braced_guid(&o.guid),
                    "class": world.class_name(o.class).or_else(|| known.map(|k| k.class.as_str())),
                    "class_hash": format!("{:#010x}", o.class),
                    "group": o.group, "stream": world.stream_name(o.group),
                    "position": o.position, "flags": format!("{:#010x}", o.flags),
                    "listening": o.listening,
                    "name": known.map(|k| k.name.clone()),
                    "simgroup": known.map(|k| json!({"stream": k.stream, "resource": k.resource, "object": k.object})),
                })
            })
            .collect();
        out!(
            "{}",
            json!({"total": world.total, "walked": world.walked, "partial": world.partial, "objects": v})
        );
        return;
    }
    let mut shown = 0;
    for o in world.objects.iter().filter(|o| keep(o)) {
        shown += 1;
        let pos = o.position.map_or_else(
            || format!("{:>26}", "-"),
            |p| format!("{:8.2} {:8.2} {:8.2}", p[0], p[1], p[2]),
        );
        let name = names
            .and_then(|n| n.object(&o.guid))
            .map(|k| k.name.as_str())
            .unwrap_or_default();
        // The module reads the class from the game's code; for classes
        // without a cast of their own the SimGroup knows it.
        let class = world
            .class_name(o.class)
            .or_else(|| {
                names
                    .and_then(|n| n.object(&o.guid))
                    .map(|k| k.class.as_str())
            })
            .unwrap_or("?");
        out!(
            "{} {:<24} {:<34} {pos} {} {name}",
            proto::guid_hex(&o.guid),
            class,
            world.stream_name(o.group).unwrap_or("?"),
            if o.listening { "on " } else { "off" },
        );
    }
    eprintln!(
        "{shown} shown, {} matched of {} objects walked{}",
        world.total,
        world.walked,
        if world.partial {
            " (the module ran out of time: partial)"
        } else {
            ""
        }
    );
}

/// Breakpoints and the rest of stage 35 need a module of protocol 3.
fn need_debug(c: &Client) -> Result<()> {
    if !c.has_debug() {
        bail!(
            "the module in the game speaks protocol {}: breakpoints, pause, the free camera and textures need the module of this toolkit",
            c.version()
        );
    }
    Ok(())
}

/// `live profile`.
fn profile(link: &LinkArgs, reset: bool, memory: bool, sample: u32) -> Result<()> {
    if sample > proto::profile::MAX_SAMPLE_MS {
        bail!("--sample is at most {} ms", proto::profile::MAX_SAMPLE_MS);
    }
    let mut c = link.connect()?;
    if !c.has_profile() {
        bail!(
            "the module speaks protocol {}: it does not measure the game (stage 58 needs protocol 7; install the module of this toolkit)",
            c.version()
        );
    }
    let p = c.profile(reset, memory, sample)?;
    if link.json {
        out!("{}", live::profile::to_json(&p, true));
    } else {
        let text = live::profile::to_text(&p);
        out!("{}", text.trim_end());
        if reset {
            out!("(the module's rings and read counters are reset)");
        }
    }
    Ok(())
}

pub fn confirm(yes: bool, what: &str) -> Result<()> {
    if !yes {
        bail!("{what} changes the running game: add --yes to do it");
    }
    Ok(())
}

pub fn run(command: LiveCommand) -> Result<()> {
    use crate::live_state_cmd as st;
    match command {
        LiveCommand::Stream(c) => st::stream(c)?,
        LiveCommand::Time {
            at,
            rate,
            default_rate,
            yes,
            link,
        } => st::time(&link, at.as_deref(), rate, default_rate, yes)?,
        LiveCommand::Weather(link) => st::weather(&link)?,
        LiveCommand::Files(link) => st::files(&link)?,
        LiveCommand::Resources(link) => st::resources(&link)?,
        LiveCommand::Resource {
            ids,
            out,
            compare,
            link,
        } => st::resource(&link, &ids, out.as_deref(), compare.as_deref())?,
        LiveCommand::ResourceCheck {
            expected,
            streams,
            original,
            sample,
            link,
        } => st::resource_check(&link, &expected, &streams, original.as_deref(), sample)?,
        LiveCommand::Profile {
            reset,
            memory,
            sample,
            link,
        } => profile(&link, reset, memory, sample)?,
        LiveCommand::Money {
            set,
            add,
            yes,
            link,
        } => st::money(&link, set, add, yes)?,
        LiveCommand::Family(link) => st::family(&link)?,
        LiveCommand::Mission(link) => st::mission(&link)?,
        LiveCommand::Var(c) => st::var(c)?,
        LiveCommand::Object(c) => crate::live_object_cmd::run(c)?,
        LiveCommand::Status(link) => {
            let mut c = link.connect()?;
            let s = c.status()?;
            let h = c.hello().clone();
            if link.json {
                out!(
                    "{}",
                    json!({
                        "module": h.module, "protocol": h.protocol, "edition": h.edition,
                        "writes": h.writes, "frame": s.frame, "state": s.state.name(),
                        "player": s.player, "sim_time_ms": s.sim_time_ms,
                        "time_of_day": s.time_of_day,
                    })
                );
            } else {
                out!("module: {} (protocol {})", h.module, h.protocol);
                out!("edition: {}", h.edition);
                out!("writes: {}", if h.writes { "allowed" } else { "off" });
                out!("main loop: {}, frame {}", s.state.name(), s.frame);
                out!("player: {}", if s.player { "present" } else { "none" });
                if let Some(t) = s.sim_time_ms {
                    out!("game time: {:.1} s", f64::from(t) / 1000.0);
                }
                if let Some([h, m, sec]) = s.time_of_day {
                    out!("time of day: {h:02}:{m:02}:{sec:02}");
                }
            }
        }
        LiveCommand::Player(link) => {
            let p = link.connect()?.player()?;
            match (p, link.json) {
                (Some(p), true) => out!("{}", pose_json(&p)),
                (Some(p), false) => out!(
                    "{:.3} {:.3} {:.3} facing {:.4} {:.4} yaw {:.1}",
                    p.position[0],
                    p.position[1],
                    p.position[2],
                    p.forward[0],
                    p.forward[2],
                    p.yaw().to_degrees()
                ),
                (None, true) => out!("null"),
                (None, false) => bail!("no player now (menu, loading or cutscene)"),
            }
        }
        LiveCommand::Camera(link) => {
            let c = link.connect()?.camera()?;
            match (c, link.json) {
                (Some(c), true) => {
                    let mut v = pose_json(&c.pose);
                    v["fov_degrees"] = json!(c.fov);
                    out!("{v}");
                }
                (Some(c), false) => {
                    let p = c.pose;
                    out!(
                        "{:.3} {:.3} {:.3} looking {:.4} {:.4} {:.4}{}",
                        p.position[0],
                        p.position[1],
                        p.position[2],
                        p.forward[0],
                        p.forward[1],
                        p.forward[2],
                        c.fov.map(|f| format!(" fov {f:.1}")).unwrap_or_default()
                    );
                }
                (None, true) => out!("null"),
                (None, false) => bail!("no camera now"),
            }
        }
        LiveCommand::Streams(link) => {
            let list = link.connect()?.streams()?;
            if link.json {
                out!(
                    "{}",
                    json!(list.iter().map(|s| &s.name).collect::<Vec<_>>())
                );
            } else {
                for s in list {
                    out!("{}", s.name);
                }
            }
        }
        LiveCommand::Objects {
            link,
            all: true,
            class,
            max,
            logic,
            stream,
            ..
        } => {
            let names = link.names(false)?;
            let filter = WorldFilter {
                class: class.as_deref().map(class_hash),
                positioned: logic.then_some(false),
                ..WorldFilter::default()
            };
            let world = link
                .connect()?
                .world(filter, max.unwrap_or(proto::world::MAX_WORLD_OBJECTS))?;
            print_world(&world, names.as_ref(), link.json, stream.as_deref());
        }
        LiveCommand::Objects {
            link,
            radius,
            class,
            at,
            max,
            ..
        } => {
            let center = at.as_deref().map(vec3).transpose()?;
            let max = max.unwrap_or(50).min(u32::from(u16::MAX)) as u16;
            let list =
                link.connect()?
                    .objects(center, radius, class.as_deref().map(class_hash), max)?;
            if link.json {
                let v: Vec<_> = list
                    .iter()
                    .map(|o| {
                        json!({"guid": proto::guid_hex(&o.guid), "braced": proto::braced_guid(&o.guid),
                               "class": o.class_name, "class_hash": format!("{:#010x}", o.class),
                               "position": o.position, "distance": o.distance})
                    })
                    .collect();
                out!("{}", json!(v));
            } else {
                for o in list {
                    out!(
                        "{} {:<24} {:8.2} {:8.2} {:8.2}  {:.1} m",
                        proto::guid_hex(&o.guid),
                        if o.class_name.is_empty() {
                            "?"
                        } else {
                            o.class_name.as_str()
                        },
                        o.position[0],
                        o.position[1],
                        o.position[2],
                        o.distance
                    );
                }
            }
        }
        LiveCommand::Find { query, link } => {
            let guid = proto::parse_guid(&query);
            let names = link.names(guid.is_none())?;
            let guids: Vec<proto::Guid> = match (guid, &names) {
                (Some(g), _) => vec![g],
                (None, Some(n)) => n
                    .search(&query)
                    .into_iter()
                    .map(|(g, _)| g)
                    .take(4096)
                    .collect(),
                (None, None) => Vec::new(),
            };
            if guids.is_empty() {
                bail!("no object of the game's SimGroups is called like `{query}`");
            }
            let world = link.connect()?.world(
                WorldFilter {
                    guids: guids.clone(),
                    ..WorldFilter::default()
                },
                proto::world::MAX_WORLD_OBJECTS,
            )?;
            print_world(&world, names.as_ref(), link.json, None);
            let loaded = world.objects.len();
            if loaded < guids.len() && !link.json {
                eprintln!(
                    "{} of {} object(s) with that name are not in the game now (their stream is not loaded)",
                    guids.len() - loaded,
                    guids.len()
                );
            }
            if loaded == 0 {
                bail!("not in the running game");
            }
        }
        LiveCommand::Log {
            follow,
            filter,
            messages,
            errors,
            after,
            link,
        } => {
            let names = link.names(false)?;
            let kinds = if errors {
                mask::LUA_ERROR
            } else if messages {
                mask::ALL_V8
            } else {
                mask::OUTPUT | mask::LUA_ERROR
            };
            let mut c = link.connect()?;
            if !c.has_journal() {
                bail!(
                    "the module in the game speaks protocol {} and has no journal: install the module of this toolkit",
                    c.version()
                );
            }
            let want = filter.map(|f| f.to_ascii_lowercase());
            let mut next = after;
            let mut warned = 0u8;
            // Lost events are told at most every 10 s (a follow of the
            // messages loses some every poll: the per-message limit).
            let mut lost = 0u64;
            let mut told = std::time::Instant::now();
            loop {
                let b = c.events(next, kinds, 500)?;
                let kinds = mask::for_version(kinds, c.version());
                if b.recording & kinds != kinds && warned == 0 {
                    warned = 1;
                    eprintln!(
                        "warning: the module records only part of this (journal {:#x} of {kinds:#x}; journal=0 in doctor_live.cfg, or a hook could not be installed)",
                        b.recording
                    );
                }
                lost += b.lost;
                if lost > 0 && (!follow || told.elapsed() >= Duration::from_secs(10)) {
                    eprintln!("({lost} event(s) lost: the ring went round or a rate limit)");
                    lost = 0;
                    told = std::time::Instant::now();
                }
                for e in &b.events {
                    if link.json {
                        let v = event_json(e, names.as_ref());
                        if want
                            .as_ref()
                            .is_none_or(|w| v.to_string().to_ascii_lowercase().contains(w))
                        {
                            out!("{v}");
                        }
                        continue;
                    }
                    let lines = event_lines(e, names.as_ref());
                    if want
                        .as_ref()
                        .is_none_or(|w| lines.iter().any(|l| l.to_ascii_lowercase().contains(w)))
                    {
                        for l in lines {
                            out!("{l}");
                        }
                    }
                }
                next = b.next.saturating_sub(1);
                if !follow {
                    if b.events.is_empty() {
                        break;
                    }
                    continue;
                }
                std::thread::sleep(Duration::from_millis(250));
            }
        }
        LiveCommand::Teleport {
            position,
            yaw,
            yes,
            link,
        } => {
            let p = vec3(&position)?;
            confirm(yes, "teleport")?;
            let done = link.connect()?.teleport(p, yaw.map(f32::to_radians))?;
            out!("{}", done.note);
        }
        LiveCommand::Send {
            message,
            to,
            yes,
            link,
        } => {
            let target = to
                .as_deref()
                .map(|t| proto::parse_guid(t).with_context(|| format!("`{t}` is not a GUID")))
                .transpose()?;
            confirm(yes, "send")?;
            let done = link.connect()?.send(MessageId::parse(&message), target)?;
            if link.json {
                out!("{}", json!({"count": done.count, "note": done.note}));
            } else {
                out!("{} ({})", done.note, done.count);
            }
        }
        LiveCommand::Lua { code, yes, link } => {
            confirm(yes, "lua")?;
            let r = link.connect()?.lua(&code)?;
            if link.json {
                out!("{}", json!({"ok": r.ok, "values": r.values}));
            } else {
                for v in &r.values {
                    out!("{v}");
                }
                if !r.ok {
                    bail!("the Lua code failed");
                }
            }
        }
        LiveCommand::Peek { address, len, link } => {
            let a = u32::from_str_radix(address.trim_start_matches("0x"), 16)
                .with_context(|| format!("`{address}` is not a hex address"))?;
            let bytes = link.connect()?.peek(a, len)?;
            for (i, row) in bytes.chunks(16).enumerate() {
                let hex: Vec<String> = row.iter().map(|b| format!("{b:02x}")).collect();
                out!("{:08x}: {}", a as usize + i * 16, hex.join(" "));
            }
        }
        LiveCommand::Install {
            game_dir,
            writes,
            port,
            module,
            apply,
            workspace_dir,
            mods,
            cmd_txt,
            files_log,
            resources_in_memory,
            resources_dump,
        } => {
            let path = module.or_else(live::module_binary).context(
                "the module is not built: run tools/build-live-module.sh (or --module FILE)",
            )?;
            let bytes = std::fs::read(&path).with_context(|| path.display().to_string())?;
            let config = Config {
                port: port.unwrap_or_else(live::free_port),
                token: live::new_token(),
                writes,
                fix_input: true,
                journal: true,
                files_log,
                redirects: Vec::new(),
                resources: Vec::new(),
                resources_dump,
            };
            let mut config = config;
            use doctor_toolkit::play::overlay::{self, Overlay, Source};
            let mut staged = None;
            if apply || mods || cmd_txt.is_some() {
                overlay::availability(&game_dir)
                    .map_err(|why| anyhow::anyhow!("cannot redirect the game's files: {why}"))?;
                overlay::remove_stale(&game_dir)?;
                let game = doctor_toolkit::game::GameInstance::open(&game_dir)?;
                let source = if mods {
                    Some(Source::Mods)
                } else if apply {
                    Some(Source::Workspace(workspace_dir.unwrap_or_else(|| {
                        doctor_toolkit::workspace::Workspace::default_dir(&game_dir)
                    })))
                } else {
                    None
                };
                let text = cmd_txt.map(|t| format!("{}\r\n", t.trim_end()));
                let dir = overlay::default_dir(&game_dir)
                    .context("no folder for the staged files (no HOME or LOCALAPPDATA)")?;
                let built = Overlay::build_with(
                    &game,
                    source.as_ref(),
                    text.as_deref(),
                    &dir,
                    overlay::Streams::in_memory(resources_in_memory),
                )?;
                built.configure(&mut config);
                out!(
                    "staged {} file(s) in {}; the module gives them to the game",
                    config.redirects.len(),
                    dir.display()
                );
                if !config.resources.is_empty() {
                    out!(
                        "{} stream(s) are composed in memory from the game's files and resource patches",
                        config.resources.len()
                    );
                }
                staged = Some(built);
            }
            let install = ModuleInstall::write(&game_dir, &bytes, &config)?;
            // Kept until `live uninstall`.
            if let Some(o) = staged {
                o.keep();
            }
            out!(
                "installed {} (port {}, writes {})",
                game_dir.join("scripts").join(proto::MODULE_FILE).display(),
                config.port,
                if writes { "on" } else { "off" }
            );
            match live::loader::find(&game_dir) {
                Some(l) => out!(
                    "ASI loader: {}{}; under Wine it needs WINEDLLOVERRIDES={}",
                    l.file.display(),
                    if l.ultimate {
                        " (Ultimate ASI Loader)"
                    } else {
                        ""
                    },
                    l.wine_override()
                ),
                None => out!(
                    "warning: no ASI loader (dinput8.dll) in the game folder: the game will not load the module"
                ),
            }
            // Stays until `live uninstall` (or the next `play --live`).
            std::mem::forget(install);
        }
        LiveCommand::Uninstall { game_dir } => {
            if live::restore_stale(&game_dir)? {
                out!("took the module away");
            } else {
                out!("no module of the toolkit in {}", game_dir.display());
            }
            if doctor_toolkit::play::overlay::remove_stale(&game_dir)? {
                out!("removed the files staged for it");
            }
        }
        LiveCommand::Fake {
            port,
            writes,
            activity,
        } => {
            let game = live::fake::FakeGame::start_on(
                port,
                Config {
                    port,
                    token: String::new(),
                    writes,
                    fix_input: true,
                    journal: true,
                    ..Default::default()
                },
            )?;
            out!("fake game on 127.0.0.1:{} (Ctrl+C to stop)", game.port);
            let mut beat = 0u64;
            loop {
                if activity {
                    std::thread::sleep(Duration::from_secs(1));
                    game.world
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .activity(beat);
                    beat += 1;
                } else {
                    std::thread::sleep(Duration::from_secs(3600));
                }
            }
        }
        LiveCommand::Break {
            message,
            to,
            print,
            lua_error,
            once,
            add,
            clear,
            hold,
            wait,
            link,
        } => {
            let names = link.names(false)?;
            let mut c = link.connect()?;
            need_debug(&c)?;
            let target = to
                .as_deref()
                .map(|t| proto::parse_guid(t).with_context(|| format!("`{t}` is not a GUID")))
                .transpose()?;
            let mut rules: Vec<proto::BreakRule> = message
                .iter()
                .map(|m| proto::BreakRule {
                    on: proto::BreakOn::Message {
                        hash: MessageId::parse(m).hash(),
                        receiver: target,
                    },
                    once,
                })
                .chain(print.iter().map(|t| proto::BreakRule {
                    on: proto::BreakOn::Output { text: t.clone() },
                    once,
                }))
                .collect();
            if lua_error {
                rules.push(proto::BreakRule {
                    on: proto::BreakOn::LuaError,
                    once,
                });
            }
            if !rules.is_empty() || clear || hold != 0 {
                if add || (rules.is_empty() && !clear) {
                    let mut old = c.debug_state()?.rules;
                    old.extend(rules);
                    rules = old;
                }
                c.set_breaks(rules, hold)?;
            }
            let mut d = c.debug_state()?;
            if wait {
                let mut announced = false;
                while d.stop.is_none() {
                    if !announced {
                        eprintln!("waiting for the game to stop (Ctrl+C to give up)…");
                        announced = true;
                    }
                    std::thread::sleep(Duration::from_millis(200));
                    d = c.debug_state()?;
                }
            }
            if link.json {
                out!("{}", debug_json(&d, names.as_ref()));
            } else {
                for l in debug_lines(&d, names.as_ref()) {
                    out!("{l}");
                }
            }
        }
        LiveCommand::Pause(link) => {
            let mut c = link.connect()?;
            need_debug(&c)?;
            out!("{}", c.control(proto::Control::Pause)?.note);
        }
        LiveCommand::Continue { step, link } => {
            let mut c = link.connect()?;
            need_debug(&c)?;
            let control = if step {
                proto::Control::Step
            } else {
                proto::Control::Continue
            };
            out!("{}", c.control(control)?.note);
        }
        LiveCommand::Freecam {
            position,
            yaw,
            pitch,
            release,
            yes,
            link,
        } => {
            confirm(yes, "freecam")?;
            let mut c = link.connect()?;
            need_debug(&c)?;
            if release {
                out!("{}", c.set_camera(None)?.note);
                return Ok(());
            }
            let now = c
                .camera()?
                .context("the game has no camera now (not in a level)")?;
            let at = position.as_deref().map(vec3).transpose()?;
            let view = free_camera(now, at, yaw, pitch);
            let done = c.set_camera(Some(view))?;
            out!(
                "{} at ({:.2}, {:.2}, {:.2})",
                done.note,
                view.pose.position[0],
                view.pose.position[1],
                view.pose.position[2]
            );
        }
        LiveCommand::Texture {
            name,
            stream,
            tint,
            yes,
            link,
        } => {
            let dir = link
                .game
                .as_deref()
                .context("the textures come from the workspace of a game folder: add --game DIR")?;
            let want = name
                .as_deref()
                .map(|n| {
                    u32::from_str_radix(n.trim().trim_start_matches("0x"), 16)
                        .with_context(|| format!("`{n}` is not a 0x hash"))
                })
                .transpose()?;
            let textures = if let (Some(stream), Some(tint)) = (&stream, &tint) {
                let rgb = u32::from_str_radix(tint.trim_start_matches('#'), 16)
                    .ok()
                    .filter(|_| tint.trim_start_matches('#').len() == 6)
                    .with_context(|| format!("`{tint}` is not RRGGBB"))?;
                let game = doctor_toolkit::game::GameInstance::open(dir)?;
                let mut list = live::tinted_textures(
                    &game,
                    stream,
                    [(rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8],
                )?;
                list.retain(|t| want.is_none_or(|w| w == t.name));
                list
            } else {
                let ws_dir = doctor_toolkit::workspace::Workspace::default_dir(dir);
                if !doctor_toolkit::workspace::Workspace::exists(&ws_dir) {
                    bail!("{} has no workspace", dir.display());
                }
                let ws = doctor_toolkit::workspace::Workspace::open(&ws_dir)?;
                live::hot_textures(&ws, want)?
            };
            if textures.is_empty() {
                bail!(
                    "no edited texture in the workspace{}",
                    if want.is_some() {
                        " with this name"
                    } else {
                        ""
                    }
                );
            }
            confirm(yes, "texture")?;
            let mut c = link.connect()?;
            need_debug(&c)?;
            let mut failed = 0;
            for t in textures {
                match c.texture(t.name, t.data) {
                    Ok(d) => out!("{:#010x} ({}): {}", t.name, t.stream, d.note),
                    Err(e) => {
                        failed += 1;
                        out!("{:#010x} ({}): {e}", t.name, t.stream);
                    }
                }
            }
            if failed > 0 {
                bail!("{failed} texture(s) not replaced");
            }
        }
    }
    Ok(())
}

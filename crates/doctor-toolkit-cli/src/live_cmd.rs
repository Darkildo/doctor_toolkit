//! `live`: the live link with the running game (stage 34); the game's
//! journal and every object (stage 49).

use std::io::Write as _;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use clap::{Args, Subcommand};
use serde_json::json;

use doctor_toolkit::live::names::LiveNames;
use doctor_toolkit::live::proto::journal::{EventBody, MessageOrigin, mask};
use doctor_toolkit::live::proto::{self, Event, MessageId, World, WorldFilter, WorldObject};
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
    timeout: u64,
    /// Print JSON
    #[arg(long)]
    json: bool,
    /// Names of objects and messages from the SimGroups of the --game folder
    /// and its workspace (reads every SimGroup first: a few seconds)
    #[arg(long, requires = "game")]
    names: bool,
}

impl LinkArgs {
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
    fn names(&self, needed: bool) -> Result<Option<LiveNames>> {
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

    fn connect(&self) -> Result<Client> {
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
    },
    /// Take the module away from a game folder
    Uninstall { game_dir: PathBuf },
    /// Answer like the module, without the game (to try the editor and scripts)
    Fake {
        #[arg(long, default_value_t = proto::DEFAULT_PORT)]
        port: u16,
        #[arg(long)]
        writes: bool,
    },
}

impl LiveCommand {
    pub fn may_write_game(&self) -> bool {
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

/// The name of a message: from the index, else the hash.
fn message_name(names: Option<&LiveNames>, hash: u32) -> String {
    names
        .and_then(|n| n.message(hash))
        .map_or_else(|| format!("{hash:#010x}"), str::to_owned)
}

/// `{GUID} name` of an object: the name from the index when known.
fn object_label(names: Option<&LiveNames>, guid: &proto::Guid) -> String {
    match names.and_then(|n| n.object(guid)) {
        Some(o) if !o.name.is_empty() => format!("{} {}", o.name, proto::guid_hex(guid)),
        _ => proto::guid_hex(guid),
    }
}

/// The lines of one event.
fn event_lines(e: &Event, names: Option<&LiveNames>) -> Vec<String> {
    let head = format!(
        "#{} frame {} {:>8.1}s",
        e.seq,
        e.frame,
        f64::from(e.sim_time_ms) / 1000.0
    );
    match &e.body {
        EventBody::Output {
            source,
            text,
            script,
            line,
        } => {
            let at = if script.is_empty() {
                String::new()
            } else {
                format!(" {script}:{line}:")
            };
            vec![format!("{head} {}{at} {text}", source.name())]
        }
        EventBody::LuaError {
            message, traceback, ..
        } => {
            let mut out = vec![format!("{head} lua error: {message}")];
            out.extend(traceback.iter().map(|t| format!("    {t}")));
            out
        }
        EventBody::Message {
            hash,
            data,
            forced,
            listeners,
            receivers,
            origin,
        } => {
            let mut line = format!(
                "{head} message {}{}{} -> {listeners} listener(s)",
                message_name(names, *hash),
                if *data != 0 {
                    format!(" data {data:#x}")
                } else {
                    String::new()
                },
                if *forced { " forced" } else { "" },
            );
            if !receivers.is_empty() {
                let list: Vec<String> = receivers.iter().map(|g| object_label(names, g)).collect();
                line += &format!(": {}", list.join(", "));
            }
            if *origin == MessageOrigin::Link {
                line += " (live link)";
            }
            vec![line]
        }
    }
}

fn event_json(e: &Event, names: Option<&LiveNames>) -> serde_json::Value {
    let mut v = json!({"seq": e.seq, "frame": e.frame, "sim_time_ms": e.sim_time_ms});
    match &e.body {
        EventBody::Output {
            source,
            text,
            script,
            line,
        } => {
            v["kind"] = json!("output");
            v["source"] = json!(source.name());
            v["text"] = json!(text);
            if !script.is_empty() {
                v["script"] = json!(script);
                v["line"] = json!(line);
            }
        }
        EventBody::LuaError {
            status,
            message,
            chunk,
            line,
            traceback,
        } => {
            v["kind"] = json!("lua_error");
            v["status"] = json!(status);
            v["message"] = json!(message);
            v["chunk"] = json!(chunk);
            v["line"] = json!(line);
            v["traceback"] = json!(traceback);
        }
        EventBody::Message {
            hash,
            data,
            forced,
            listeners,
            receivers,
            origin,
        } => {
            v["kind"] = json!("message");
            v["hash"] = json!(format!("{hash:#010x}"));
            v["name"] = json!(names.and_then(|n| n.message(*hash)));
            v["data"] = json!(data);
            v["forced"] = json!(forced);
            v["listeners"] = json!(listeners);
            v["receivers"] = json!(
                receivers
                    .iter()
                    .map(|g| json!({
                        "guid": proto::guid_hex(g),
                        "name": names.and_then(|n| n.object(g)).map(|o| o.name.clone()),
                    }))
                    .collect::<Vec<_>>()
            );
            v["by_link"] = json!(*origin == MessageOrigin::Link);
        }
    }
    v
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

fn confirm(yes: bool, what: &str) -> Result<()> {
    if !yes {
        bail!("{what} changes the running game: add --yes to do it");
    }
    Ok(())
}

pub fn run(command: LiveCommand) -> Result<()> {
    match command {
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
                mask::ALL
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
            };
            let install = ModuleInstall::write(&game_dir, &bytes, &config)?;
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
        }
        LiveCommand::Fake { port, writes } => {
            let game = live::fake::FakeGame::start_on(
                port,
                Config {
                    port,
                    token: String::new(),
                    writes,
                    fix_input: true,
                    journal: true,
                },
            )?;
            out!("fake game on 127.0.0.1:{} (Ctrl+C to stop)", game.port);
            loop {
                std::thread::sleep(Duration::from_secs(3600));
            }
        }
    }
    Ok(())
}

//! `live time`, `weather`, `stream`, `money`, `family`, `mission`, `var`:
//! the world and the state of the game through the live link (stage 48).

use std::io::Write as _;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use clap::Subcommand;
use serde_json::json;

use doctor_toolkit::live::Client;
use doctor_toolkit::live::names::LiveNames;
use doctor_toolkit::live::proto::{self, MessageId, MoneyChange, StreamAction, TimeChange, state};

use crate::live_cmd::{LinkArgs, confirm, object_label};

macro_rules! out {
    ($($arg:tt)*) => {
        if writeln!(std::io::stdout(), $($arg)*).is_err() {
            std::process::exit(0);
        }
    };
}

/// `live resources`: the streams the module composes in memory from the
/// game's files and resource patches (stage 100).
pub fn resources(link: &LinkArgs) -> Result<()> {
    let mut c = link.connect()?;
    if !c.has_resources() {
        bail!(
            "the module speaks protocol {}: it composes no streams (stage 100 needs protocol 9)",
            c.version()
        );
    }
    let r = c.resources()?;
    if link.json {
        out!(
            "{}",
            json!({
                "hooked": r.hooked,
                "total": r.total,
                "open": r.open,
                "streams": r.streams.iter().map(|s| json!({
                    "from": s.from, "to": s.to, "opens": s.opens, "failed": s.failed,
                    "replaced": s.replaced, "added": s.added,
                    "original_len": s.original_len, "virtual_len": s.virtual_len,
                    "data_size": s.data_size, "compose_micros": s.compose_micros,
                    "bytes_read": s.bytes_read, "error": s.error,
                })).collect::<Vec<_>>(),
            })
        );
        return Ok(());
    }
    if r.total == 0 {
        out!("the module composes no streams (no `resources=` lines in doctor_live.cfg)");
        return Ok(());
    }
    out!(
        "{} stream(s) with a resource patch, {} opened by the game, {} open now; hooks {}",
        r.total,
        r.streams.iter().filter(|s| s.opens > 0).count(),
        r.open,
        if r.hooked { "on" } else { "OFF" }
    );
    for s in &r.streams {
        if s.opens == 0 {
            out!("  {}: not opened yet", s.from);
            continue;
        }
        out!(
            "  {}: {}x, {} replaced, {} added, {} -> {} bytes, data size {}, composed in {} us, {} bytes read{}{}",
            s.from,
            s.opens,
            s.replaced,
            s.added,
            s.original_len,
            s.virtual_len,
            s.data_size,
            s.compose_micros,
            s.bytes_read,
            if s.failed > 0 {
                format!(", {} open(s) NOT composed", s.failed)
            } else {
                String::new()
            },
            if s.error.is_empty() {
                String::new()
            } else {
                format!(" — {}", s.error)
            }
        );
    }
    if (r.streams.len() as u32) < r.total {
        out!("  … and {} more", r.total - r.streams.len() as u32);
    }
    Ok(())
}

/// The four ids of a resource: one hex number or four joined by dots.
fn resource_ids(text: &str) -> Result<[u32; 4]> {
    let hex = |s: &str| {
        u32::from_str_radix(s.trim_start_matches("0x"), 16)
            .with_context(|| format!("`{s}` is not a hex number"))
    };
    let parts: Vec<&str> = text.split('.').collect();
    match parts.as_slice() {
        [one] => Ok([hex(one)?; 4]),
        [a, b, c, d] => Ok([hex(a)?, hex(b)?, hex(c)?, hex(d)?]),
        _ => bail!("`{text}`: expected one hex id or four joined by dots"),
    }
}

/// `live resource IDS`: a loaded resource in the game's table of resources;
/// its data in memory saved (`out`) or compared with a file (`compare`).
pub fn resource(
    link: &LinkArgs,
    ids: &str,
    out_file: Option<&std::path::Path>,
    compare: Option<&std::path::Path>,
) -> Result<()> {
    let ids = resource_ids(ids)?;
    let mut c = link.connect()?;
    if !c.has_resources() {
        bail!(
            "the module speaks protocol {}: it does not read the table of resources (protocol 9)",
            c.version()
        );
    }
    let Some(l) = c.loaded_resource(ids)? else {
        if link.json {
            out!("{}", json!({ "loaded": false }));
            return Ok(());
        }
        bail!("no loaded resource has these ids (its stream is not loaded)");
    };
    let mut data = Vec::new();
    if out_file.is_some() || compare.is_some() {
        data.reserve(l.size as usize);
        while (data.len() as u32) < l.size {
            let n = (l.size - data.len() as u32).min(0x8000) as u16;
            let chunk = c
                .peek(l.data + data.len() as u32, n)
                .context("cannot read the data of the resource")?;
            if chunk.is_empty() {
                bail!("the module read nothing at {:#010x}", l.data);
            }
            data.extend_from_slice(&chunk);
        }
        if let Some(path) = out_file {
            std::fs::write(path, &data).with_context(|| path.display().to_string())?;
        }
    }
    let compared = match compare {
        None => None,
        Some(path) => {
            let want = std::fs::read(path).with_context(|| path.display().to_string())?;
            let differing = data.iter().zip(&want).filter(|(a, b)| a != b).count();
            let first = data.iter().zip(&want).position(|(a, b)| a != b);
            Some((want.len(), differing, first))
        }
    };
    if link.json {
        out!(
            "{}",
            json!({
                "loaded": true,
                "record": l.record, "data": l.data, "size": l.size,
                "refs": l.refs, "flags": l.flags,
                "compared": compared.map(|(len, differing, first)| json!({
                    "file_len": len, "differing_bytes": differing, "first_difference": first,
                    "same": differing == 0 && len == l.size as usize,
                })),
            })
        );
        return Ok(());
    }
    out!(
        "record {:#010x}: data {:#010x}, {} bytes, {} reference(s), flags {:#06x}",
        l.record,
        l.data,
        l.size,
        l.refs,
        l.flags
    );
    if let Some(path) = out_file {
        out!("saved {} bytes to {}", data.len(), path.display());
    }
    if let Some((len, differing, first)) = compared {
        if differing == 0 && len == l.size as usize {
            out!("same as the file: {len} bytes");
        } else {
            out!(
                "DIFFERS from the file: {} bytes in memory, {len} in the file, {differing} differing byte(s){}",
                l.size,
                first.map_or(String::new(), |at| format!(", the first at {at:#x}"))
            );
        }
    }
    Ok(())
}

/// What the comparison of one resource in memory gave.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum InMemory {
    Same,
    Differs,
    OtherSize,
    NotLoaded,
    /// The record is there with its size, the pointer to the data is null:
    /// the game gave the image back after the loader of the type took it
    /// (textures go to the graphics card).
    Released,
    Unreadable,
}

/// Compares the first `limit` bytes of `want` with the data of the loaded
/// resource `ids`; the number of differing bytes and the first of them.
fn compare_loaded(
    c: &mut Client,
    ids: [u32; 4],
    want: &[u8],
    limit: usize,
) -> Result<(InMemory, usize, Option<usize>, Option<proto::Loaded>)> {
    let Some(l) = c.loaded_resource(ids)? else {
        return Ok((InMemory::NotLoaded, 0, None, None));
    };
    if l.size as usize != want.len() {
        return Ok((InMemory::OtherSize, 0, None, Some(l)));
    }
    if l.data == 0 {
        return Ok((InMemory::Released, 0, None, Some(l)));
    }
    let len = want.len().min(limit);
    let mut differing = 0usize;
    let mut first = None;
    let mut at = 0usize;
    while at < len {
        let n = (len - at).min(0x8000);
        let Ok(chunk) = c.peek(l.data + at as u32, n as u16) else {
            return Ok((InMemory::Unreadable, differing, first, Some(l)));
        };
        if chunk.len() != n {
            return Ok((InMemory::Unreadable, differing, first, Some(l)));
        }
        for (i, (a, b)) in chunk.iter().zip(&want[at..at + n]).enumerate() {
            if a != b {
                differing += 1;
                first.get_or_insert(at + i);
            }
        }
        at += n;
    }
    let state = if differing == 0 {
        InMemory::Same
    } else {
        InMemory::Differs
    };
    Ok((state, differing, first, Some(l)))
}

/// `live resource-check`: the resources in the game's memory against the
/// streams of a folder (what `apply` writes).
pub fn resource_check(
    link: &LinkArgs,
    expected: &std::path::Path,
    streams: &[String],
    original: Option<&std::path::Path>,
    sample: u32,
) -> Result<()> {
    use doctor_toolkit::game::GameInstance;
    use doctor_toolkit::workspace::ResourceKey;
    use std::collections::BTreeMap;
    let mut c = link.connect()?;
    if !c.has_resources() {
        bail!(
            "the module speaks protocol {}: it does not read the table of resources (protocol 9)",
            c.version()
        );
    }
    let want = GameInstance::open(expected)?;
    let base = original.map(GameInstance::open).transpose()?;
    let mut report = Vec::new();
    let mut total: BTreeMap<InMemory, usize> = BTreeMap::new();
    for name in streams {
        let index = want
            .find_stream(name)
            .with_context(|| format!("no stream {name} in {}", expected.display()))?;
        let file = want.open_stream(index)?;
        // The resources of the unchanged game by key: type and ids.
        let before = match &base {
            None => None,
            Some(game) => Some(match game.find_stream(name) {
                None => BTreeMap::new(),
                Some(i) => {
                    let f = game.open_stream(i)?;
                    let mut map = BTreeMap::new();
                    for (n, r) in f.layout().resources.iter().enumerate() {
                        map.insert((r.header.type_id, r.header.ids), f.resource_data(n)?);
                    }
                    map
                }
            }),
        };
        // By type: resources that are the same, differ (or cannot be
        // read), are released, are absent.
        let mut by_type: BTreeMap<String, [usize; 4]> = BTreeMap::new();
        let mut changed = Vec::new();
        for (n, r) in file.layout().resources.iter().enumerate() {
            let data = file.resource_data(n)?;
            let key = ResourceKey::new(name, &r.header);
            let state_before = before
                .as_ref()
                .map(|m| m.get(&(r.header.type_id, r.header.ids)));
            let is_changed = match state_before {
                None => true,
                Some(None) => true,
                Some(Some(old)) => *old != data,
            };
            // A resource without ids (a SimGroup) is not in the table.
            if r.header.ids == [0; 4] {
                if is_changed && before.is_some() {
                    changed.push(json!({
                        "key": key.to_string(), "size": data.len(),
                        "added": matches!(state_before, Some(None)),
                        "state": "no ids: not in the table of resources",
                    }));
                }
                continue;
            }
            let full = is_changed && before.is_some();
            let limit = if full || before.is_none() && sample == 0 {
                usize::MAX
            } else {
                sample as usize
            };
            let (state, differing, first, loaded) =
                compare_loaded(&mut c, r.header.ids, &data, limit)?;
            *total.entry(state).or_default() += 1;
            let slot = match state {
                InMemory::Same => 0,
                InMemory::Differs | InMemory::OtherSize | InMemory::Unreadable => 1,
                InMemory::Released => 2,
                InMemory::NotLoaded => 3,
            };
            by_type.entry(key.type_name()).or_default()[slot] += 1;
            if full {
                changed.push(json!({
                    "key": key.to_string(),
                    "size": data.len(),
                    "added": matches!(state_before, Some(None)),
                    "old_size": state_before.flatten().map(Vec::len),
                    "state": match state {
                        InMemory::Same => "same",
                        InMemory::Differs => "differs",
                        InMemory::OtherSize => "other size",
                        InMemory::NotLoaded => "not loaded",
                        InMemory::Released => "in the table with this size; its data went to the loader of its type (null pointer)",
                        InMemory::Unreadable => "unreadable",
                    },
                    "differing_bytes": differing,
                    "first_difference": first,
                    "size_in_memory": loaded.map(|l| l.size),
                    "data": loaded.map(|l| format!("{:#010x}", l.data)),
                    "refs": loaded.map(|l| l.refs),
                }));
            }
        }
        report.push(json!({
            "stream": name,
            "resources": file.layout().resources.len(),
            "changed": changed,
            "by_type": by_type.iter().map(|(t, n)| json!({
                "type": t, "same": n[0], "differs": n[1], "released": n[2],
                "not_loaded": n[3],
            })).collect::<Vec<_>>(),
        }));
    }
    if link.json {
        out!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }
    for s in &report {
        out!(
            "{}: {} resource(s)",
            s["stream"].as_str().unwrap_or(""),
            s["resources"]
        );
        for r in s["changed"].as_array().into_iter().flatten() {
            out!(
                "  {} {} ({} bytes{}): {}{}",
                if r["added"] == true {
                    "added"
                } else {
                    "changed"
                },
                r["key"].as_str().unwrap_or(""),
                r["size"],
                match r["old_size"].as_u64() {
                    Some(old) if Some(old) != r["size"].as_u64() => format!(", was {old}"),
                    _ => String::new(),
                },
                r["state"].as_str().unwrap_or(""),
                match r["differing_bytes"].as_u64() {
                    Some(n) if n > 0 => format!(
                        ", {n} differing byte(s), the first at {:#x}",
                        r["first_difference"].as_u64().unwrap_or(0)
                    ),
                    _ => String::new(),
                }
            );
        }
        for t in s["by_type"].as_array().into_iter().flatten() {
            out!(
                "  type {}: {} same, {} differ, {} handed over, {} not loaded",
                t["type"].as_str().unwrap_or(""),
                t["same"],
                t["differs"],
                t["released"],
                t["not_loaded"]
            );
        }
    }
    out!(
        "in all: {} same, {} differ, {} of another size, {} with a null data pointer, {} not loaded, {} unreadable",
        total.get(&InMemory::Same).copied().unwrap_or(0),
        total.get(&InMemory::Differs).copied().unwrap_or(0),
        total.get(&InMemory::OtherSize).copied().unwrap_or(0),
        total.get(&InMemory::Released).copied().unwrap_or(0),
        total.get(&InMemory::NotLoaded).copied().unwrap_or(0),
        total.get(&InMemory::Unreadable).copied().unwrap_or(0)
    );
    Ok(())
}

/// `live stream …`
#[derive(Subcommand)]
pub enum StreamCommand {
    /// Loaded streams and the requests of the link
    List(LinkArgs),
    /// Ask the game for a stream, the way `LoadStream` does (a write: needs
    /// --yes). One request at a time: refused while the previous one is
    /// not loaded
    Load {
        /// gf2/cuba/… (with / or \, with or without .str)
        name: String,
        /// Wait until the game has loaded it
        #[arg(long)]
        wait: bool,
        #[arg(long)]
        yes: bool,
        #[command(flatten)]
        link: LinkArgs,
    },
    /// Unload a stream the link loaded (a write: needs --yes)
    Unload {
        name: String,
        /// Also a stream the game loaded itself (its objects go away under
        /// the game's feet: only for far leaf zones)
        #[arg(long)]
        force: bool,
        #[arg(long)]
        wait: bool,
        #[arg(long)]
        yes: bool,
        #[command(flatten)]
        link: LinkArgs,
    },
}

/// `live var …`
#[derive(Subcommand)]
pub enum VarCommand {
    /// Variables of the story: by name (or 0x hash), those whose name
    /// contains --find (needs --game), or all of the loaded streams
    Get {
        #[arg(value_name = "NAME")]
        vars: Vec<String>,
        /// Names that contain this (from the variabledictionary resources
        /// of --game)
        #[arg(long)]
        find: Option<String>,
        #[command(flatten)]
        link: LinkArgs,
    },
    /// Set a variable and wake those who watch it, as VariableOperator
    /// does (a write: needs --yes)
    Set {
        /// Name (or 0x hash of the name or of the record)
        name: String,
        #[arg(allow_hyphen_values = true)]
        value: i32,
        #[arg(long)]
        yes: bool,
        #[command(flatten)]
        link: LinkArgs,
    },
}

/// The world and the state of the game need a module of protocol 4.
fn need_state(c: &Client) -> Result<()> {
    if !c.has_state() {
        bail!(
            "the module in the game speaks protocol {}: time, weather, streams, money, family, missions and variables need the module of this toolkit",
            c.version()
        );
    }
    Ok(())
}

fn connect(link: &LinkArgs) -> Result<Client> {
    let c = link.connect()?;
    need_state(&c)?;
    Ok(c)
}

/// `HH:MM` or `HH:MM:SS`.
pub fn parse_time(text: &str) -> Result<[u8; 3]> {
    let parts: Vec<&str> = text.trim().split(':').collect();
    let num = |s: &str, max: u8| -> Result<u8> {
        let v: u8 = s
            .parse()
            .with_context(|| format!("`{text}` is not HH:MM[:SS]"))?;
        if v > max {
            bail!("`{text}` is not a time of day");
        }
        Ok(v)
    };
    match parts.as_slice() {
        [h, m] => Ok([num(h, 23)?, num(m, 59)?, 0]),
        [h, m, s] => Ok([num(h, 23)?, num(m, 59)?, num(s, 59)?]),
        _ => bail!("`{text}` is not HH:MM[:SS]"),
    }
}

fn time_line(t: &proto::TimeOfDay) -> String {
    format!(
        "{:02}:{:02}:{:02}, day {} of {}, {} game seconds a second (level: {})",
        t.hour, t.minute, t.second, t.day, t.year, t.rate, t.default_rate
    )
}

fn time_json(t: &proto::TimeOfDay) -> serde_json::Value {
    json!({
        "hour": t.hour, "minute": t.minute, "second": t.second,
        "day": t.day, "year": t.year, "rate": t.rate, "default_rate": t.default_rate,
    })
}

/// `live time`: print, or set the time and the rate.
pub fn time(
    link: &LinkArgs,
    at: Option<&str>,
    rate: Option<f32>,
    default_rate: bool,
    yes: bool,
) -> Result<()> {
    let change = TimeChange {
        time: at.map(parse_time).transpose()?,
        rate: if default_rate { Some(-1.0) } else { rate },
    };
    if change.rate.is_some_and(|r| !r.is_finite()) {
        bail!("the rate is not a number");
    }
    let mut c = connect(link)?;
    if change.time.is_some() || change.rate.is_some() {
        confirm(yes, "time")?;
        let d = c.set_time(change)?;
        if !link.json {
            out!("{}", d.note);
        }
    }
    match (c.time_of_day()?, link.json) {
        (Some(t), true) => out!("{}", time_json(&t)),
        (Some(t), false) => out!("{}", time_line(&t)),
        (None, true) => out!("null"),
        (None, false) => out!("no clock: not in a level"),
    }
    Ok(())
}

/// `live files`.
pub fn files(link: &LinkArgs) -> Result<()> {
    let mut c = link.connect()?;
    if !c.has_files() {
        bail!(
            "the module speaks protocol {}: it redirects no files (stage 46 needs protocol 5)",
            c.version()
        );
    }
    let f = c.files()?;
    if link.json {
        out!(
            "{}",
            json!({
                "hooked": f.hooked,
                "logging": f.logging,
                "total": f.total,
                "used": f.used(),
                "passed": f.passed,
                "redirects": f.redirects.iter().map(|r| json!({
                    "from": r.from, "to": r.to, "opens": r.opens, "failed": r.failed,
                })).collect::<Vec<_>>(),
            })
        );
        return Ok(());
    }
    if !f.hooked {
        out!("the module redirects no files (none listed in doctor_live.cfg)");
        return Ok(());
    }
    out!(
        "{} file(s) redirected, {} opened by the game; {} other open(s) in the game folder",
        f.total,
        f.used(),
        f.passed
    );
    for r in &f.redirects {
        let failed = if r.failed > 0 {
            format!(", {} failed", r.failed)
        } else {
            String::new()
        };
        out!("  {} -> {} ({}x{failed})", r.from, r.to, r.opens);
    }
    if (f.redirects.len() as u32) < f.total {
        out!("  … and {} more", f.total - f.redirects.len() as u32);
    }
    Ok(())
}

pub fn weather(link: &LinkArgs) -> Result<()> {
    let w = connect(link)?.weather()?;
    if link.json {
        out!(
            "{}",
            json!({
                "rain": w.rain.map(|r| json!({"flags": r.flags, "raining": r.flags & 1 != 0, "length": r.length, "elapsed": r.elapsed})),
                "time": w.time.as_ref().map(time_json),
            })
        );
        return Ok(());
    }
    match w.rain {
        Some(r) => out!(
            "rain: {} (flags {:#x}, {:.0} of {:.0} s)",
            if r.flags & 1 != 0 { "falling" } else { "none" },
            r.flags,
            r.elapsed,
            r.length
        ),
        None => {
            out!("rain: the level has no WeatherManager (none of the game's SimGroups makes one)")
        }
    }
    if let Some(t) = &w.time {
        out!("time of day: {}", time_line(t));
    }
    out!(
        "sky, fog and light follow the time of day (TODSequencer keys): change them with `live time`"
    );
    Ok(())
}

fn request_line(r: &proto::StreamRequest) -> String {
    format!(
        "{} {} — {} (id {:#x}, state {}, {} frame(s))",
        match r.action {
            StreamAction::Load => "load",
            StreamAction::Unload => "unload",
        },
        r.name,
        if r.done {
            "done"
        } else if r.expired {
            "gave up waiting"
        } else {
            "in progress"
        },
        r.id,
        r.state,
        r.frames
    )
}

/// Waits until the newest request of the link is done (at most `limit`).
fn wait_stream(c: &mut Client, limit: Duration) -> Result<proto::StreamRequest> {
    let start = Instant::now();
    loop {
        let q = c.stream_queue()?;
        if let Some(r) = q.requests.last()
            && (r.done || start.elapsed() > limit)
        {
            if !r.done {
                bail!("not done after {} s: {}", limit.as_secs(), request_line(r));
            }
            return Ok(r.clone());
        }
        if q.requests.is_empty() {
            bail!("the module has no stream request");
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

pub fn stream(command: StreamCommand) -> Result<()> {
    match command {
        StreamCommand::List(link) => {
            let mut c = connect(&link)?;
            let loaded = c.streams()?;
            let q = c.stream_queue()?;
            if link.json {
                out!(
                    "{}",
                    json!({
                        "loaded": loaded.iter().map(|s| &s.name).collect::<Vec<_>>(),
                        "busy": q.busy,
                        "requests": q.requests.iter().map(|r| json!({
                            "name": r.name, "load": r.action == StreamAction::Load,
                            "id": r.id, "state": r.state, "done": r.done, "frames": r.frames, "expired": r.expired,
                        })).collect::<Vec<_>>(),
                    })
                );
                return Ok(());
            }
            for s in &loaded {
                out!("{}", s.name);
            }
            eprintln!(
                "{} stream(s) loaded{}",
                loaded.len(),
                if q.busy {
                    "; the game is loading a stream now"
                } else {
                    ""
                }
            );
            for r in &q.requests {
                out!("link: {}", request_line(r));
            }
        }
        StreamCommand::Load {
            name,
            wait,
            yes,
            link,
        } => {
            confirm(yes, "stream load")?;
            let mut c = connect(&link)?;
            let d = c.stream_load(&name)?;
            out!("{}", d.note);
            if wait && d.count > 0 {
                let r = wait_stream(&mut c, Duration::from_secs(link.timeout.max(5)))?;
                out!("{}", request_line(&r));
            }
        }
        StreamCommand::Unload {
            name,
            force,
            wait,
            yes,
            link,
        } => {
            confirm(yes, "stream unload")?;
            let mut c = connect(&link)?;
            let d = c.stream_unload(&name, force)?;
            out!("{}", d.note);
            if wait {
                let r = wait_stream(&mut c, Duration::from_secs(link.timeout.max(5)))?;
                out!("{}", request_line(&r));
            }
        }
    }
    Ok(())
}

pub fn money(link: &LinkArgs, set: Option<f32>, add: Option<f32>, yes: bool) -> Result<()> {
    let mut c = connect(link)?;
    let change = match (set, add) {
        (Some(v), _) => Some(MoneyChange::Set(v)),
        (None, Some(v)) => Some(MoneyChange::Add(v)),
        _ => None,
    };
    if let Some(change) = change {
        confirm(yes, "money")?;
        let d = c.set_money(change)?;
        if !link.json {
            out!("{}", d.note);
        }
    }
    match (c.money()?, link.json) {
        (Some(m), true) => out!("{}", json!({"cash": m.cash, "books": m.books})),
        (Some(m), false) => {
            out!("money: {}", m.cash);
            if m.books.iter().any(|b| *b != 0.0) {
                let list: Vec<String> = m
                    .books
                    .iter()
                    .enumerate()
                    .filter(|(_, b)| **b != 0.0)
                    .map(|(i, b)| format!("{i}: {b}"))
                    .collect();
                out!("books by category: {}", list.join(", "));
            }
        }
        (None, true) => out!("null"),
        (None, false) => out!("no family of the player: not in a level"),
    }
    Ok(())
}

/// Name of a family by its hash (`Family+0x54` is `sdbm` of attribute 1 of
/// its object; the 28 `Family` objects of `gf2/globals_global`).
fn family_name(hash: u32) -> String {
    const NAMES: [&str; 28] = [
        "innocent",
        "worker",
        "miami_law",
        "corleone",
        "rosato_carmine",
        "rosato_tony",
        "granados",
        "almeida",
        "neutral",
        "mangano",
        "batista",
        "recruit",
        "worker_sex",
        "cuban_rebel",
        "roth",
        "worker_industrial",
        "worker_food",
        "worker_hospitality",
        "worker_racket",
        "worker_whitecollar",
        "cuban_rioter",
        "cuban_law",
        "security",
        "queens_law",
        "striker",
        "almeida_m4",
        "cuban_fake_police",
        "security_roth",
    ];
    NAMES
        .iter()
        .find(|n| proto::name_hash(n) == hash)
        .map_or_else(|| format!("{hash:#010x}"), |n| (*n).to_owned())
}

pub fn family(link: &LinkArgs) -> Result<()> {
    let names = link.names(false)?;
    let f = connect(link)?.family()?;
    if link.json {
        out!(
            "{}",
            json!({
                "families": f.families.iter().map(|x| json!({"hash": format!("{:#010x}", x.hash), "name": family_name(x.hash), "cash": x.cash})).collect::<Vec<_>>(),
                "crew": f.crew.iter().map(|s| json!({
                    "slot": s.slot, "open": s.open(), "taken": s.taken(),
                    "rank": state::rank_name(s.rank),
                    "member": s.member.map(|g| proto::guid_hex(&g)),
                    "member_name": s.member.and_then(|g| names.as_ref().and_then(|n| n.object(&g)).map(|o| o.name.clone())),
                    "member_rank": state::rank_name(s.member_rank),
                    "specialties": state::specialty_names(s.specialties),
                })).collect::<Vec<_>>(),
            })
        );
        return Ok(());
    }
    out!("crew:");
    for s in &f.crew {
        let who = match &s.member {
            Some(g) => format!(
                "{} — {}, {}",
                object_label(names.as_ref(), g),
                state::rank_name(s.member_rank),
                {
                    let sp = state::specialty_names(s.specialties);
                    if sp.is_empty() {
                        "no specialty".to_owned()
                    } else {
                        sp.join(", ")
                    }
                }
            ),
            None if s.open() => "free".into(),
            None => "locked".into(),
        };
        out!("  {} ({:<9}) {who}", s.slot + 1, state::rank_name(s.rank));
    }
    out!("families:");
    for x in &f.families {
        out!("  {:<12} {}", family_name(x.hash), x.cash);
    }
    Ok(())
}

pub fn mission(link: &LinkArgs) -> Result<()> {
    let names = link.names(false)?;
    let m = connect(link)?.mission()?;
    let label = |c: &proto::Checkpoint| {
        format!(
            "cp_{} (chapter {}) {}",
            c.number,
            c.chapter,
            object_label(names.as_ref(), &c.guid)
        )
    };
    if link.json {
        let one = |c: &proto::Checkpoint| {
            json!({"number": c.number, "chapter": c.chapter, "guid": proto::guid_hex(&c.guid),
                   "name": names.as_ref().and_then(|n| n.object(&c.guid)).map(|o| o.name.clone())})
        };
        out!(
            "{}",
            json!({"current": m.current.as_ref().map(one), "checkpoints": m.checkpoints.iter().map(one).collect::<Vec<_>>()})
        );
        return Ok(());
    }
    match &m.current {
        Some(c) => out!("current checkpoint: {}", label(c)),
        None => out!("current checkpoint: none (a start straight into a level has none)"),
    }
    out!(
        "{} checkpoint(s) in the loaded streams:",
        m.checkpoints.len()
    );
    for c in &m.checkpoints {
        out!("  {}", label(c));
    }
    Ok(())
}

/// The hash a variable is asked by: `0x…`, else `sdbm(lower(name))`.
pub fn variable_hash(text: &str) -> u32 {
    match MessageId::parse(text) {
        MessageId::Hash(h) => h,
        MessageId::Name(n) => proto::name_hash(&n),
    }
}

fn variable_name(names: Option<&LiveNames>, v: &proto::Variable) -> String {
    names
        .and_then(|n| n.variable(v.name_hash).or_else(|| n.variable(v.hash)))
        .map_or_else(|| format!("{:#010x}", v.name_hash), |n| n.name.clone())
}

pub fn var(command: VarCommand) -> Result<()> {
    match command {
        VarCommand::Get { vars, find, link } => {
            let index = link.names(find.is_some())?;
            let mut hashes: Vec<u32> = vars.iter().map(|n| variable_hash(n)).collect();
            if let (Some(f), Some(ix)) = (&find, &index) {
                hashes.extend(ix.search_variables(f).into_iter().map(|(h, _)| h));
                if hashes.is_empty() {
                    bail!("no variable name contains `{f}`");
                }
            }
            let list = connect(&link)?.variables(hashes.clone())?;
            if link.json {
                out!(
                    "{}",
                    json!(
                        list.iter()
                            .map(|v| json!({
                                "name": index.as_ref().and_then(|n| n.variable(v.name_hash)).map(|x| x.name.clone()),
                                "name_hash": format!("{:#010x}", v.name_hash),
                                "record": format!("{:#010x}", v.hash),
                                "value": v.value, "saved": v.kind & 1 != 0,
                            }))
                            .collect::<Vec<_>>()
                    )
                );
                return Ok(());
            }
            for v in &list {
                out!(
                    "{:<40} {:>8}  {}",
                    variable_name(index.as_ref(), v),
                    v.value,
                    if v.kind & 1 != 0 { "saved" } else { "" }
                );
            }
            eprintln!("{} variable(s)", list.len());
            if !vars.is_empty() && list.len() < vars.len() {
                bail!("some variables are not in the loaded streams");
            }
        }
        VarCommand::Set {
            name,
            value,
            yes,
            link,
        } => {
            confirm(yes, "var set")?;
            let mut c = connect(&link)?;
            let d = c.set_variable(variable_hash(&name), value)?;
            out!("{name}: {}", d.note);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_and_hashes() {
        assert_eq!(parse_time("22:30").unwrap(), [22, 30, 0]);
        assert_eq!(parse_time("06:05:09").unwrap(), [6, 5, 9]);
        assert!(parse_time("24:00").is_err());
        assert!(parse_time("12").is_err());
        assert_eq!(variable_hash("global_tutorial_payday"), 0x9381_3AB9);
        assert_eq!(variable_hash("0x001A01D2"), 0x001A_01D2);
        assert_eq!(family_name(0x0637_B907), "corleone");
    }
}

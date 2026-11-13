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

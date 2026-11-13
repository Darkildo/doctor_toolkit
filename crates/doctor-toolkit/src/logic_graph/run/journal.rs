//! The journal of a run — who sent what to whom and when — and its
//! comparison with a journal of the game (live link, stage 49).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::hash::string_id;

/// Who sent or received a message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Who {
    /// `object`, `engine` (the game code), `input` (an event given to the
    /// run), `lua` (the scripts' receivers).
    pub kind: String,
    /// Name, or class and index.
    pub label: String,
    /// Index in [`super::Run::actors`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guid: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub class: Option<String>,
}

impl Who {
    pub fn engine() -> Self {
        Self {
            kind: "engine".into(),
            label: "game code".into(),
            actor: None,
            guid: None,
            class: None,
        }
    }

    pub fn input() -> Self {
        Self {
            kind: "input".into(),
            label: "input".into(),
            actor: None,
            guid: None,
            class: None,
        }
    }

    pub fn lua(script: Option<&str>) -> Self {
        Self {
            kind: "lua".into(),
            label: script.map_or_else(|| "Lua".to_owned(), |s| format!("Lua {s}")),
            actor: None,
            guid: None,
            class: None,
        }
    }
}

/// How an object took a message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Took {
    /// The class model reacted (or knows it does nothing).
    Known,
    /// A black box: the message arrived, the reaction is not known.
    Unknown,
    /// A reaction given by the user.
    Manual,
}

/// What happened.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum What {
    /// A message went through the bus.
    Send {
        from: Who,
        message: String,
        /// `sdbm(lower(message))`, hexadecimal.
        hash: String,
        /// Everyone that hears it, in the order they get it.
        to: Vec<Who>,
        /// Sent to one object only (`MessageRelay` with a target).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target: Option<String>,
        /// The game code listens to it (an engine message nobody in the
        /// scope hears).
        #[serde(default)]
        engine: bool,
        /// Depth of synchronous sends (0 — from a timer or an input).
        depth: u32,
    },
    /// A message will be sent later.
    Schedule {
        from: Who,
        message: String,
        due: f64,
    },
    /// An object got a message.
    Deliver {
        to: Who,
        message: String,
        from: Who,
        took: Took,
        reaction: String,
    },
    /// A game variable changed.
    Variable {
        name: String,
        hash: String,
        old: i32,
        value: i32,
        by: Who,
    },
    /// An input of the run.
    Input { text: String },
    /// What a Lua script did besides messages and variables (calls,
    /// probes, errors).
    Lua { text: String },
    /// Something went wrong: a loop without delay, a missing script.
    Warning { text: String },
}

/// One entry of the journal.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    /// Number of the entry: 1, 2, …
    pub seq: u64,
    /// Number of the step that made it.
    pub step: u64,
    /// Game time, seconds.
    pub time: f64,
    #[serde(flatten)]
    pub what: What,
}

impl Entry {
    /// One line of text.
    pub fn line(&self) -> String {
        let t = format!("{:>9.3}", self.time);
        match &self.what {
            What::Send {
                from,
                message,
                to,
                target,
                engine,
                ..
            } => {
                let to = if to.is_empty() {
                    if *engine {
                        "nobody in the scope (an engine message)".to_owned()
                    } else {
                        "nobody".to_owned()
                    }
                } else {
                    to.iter()
                        .map(|w| w.label.clone())
                        .collect::<Vec<_>>()
                        .join(", ")
                };
                let target = target
                    .as_ref()
                    .map(|g| format!(" (only {g})"))
                    .unwrap_or_default();
                format!("{t}  {} → {message}{target} → {to}", from.label)
            }
            What::Schedule { from, message, due } => {
                format!("{t}  {} schedules {message} at {due:.3}", from.label)
            }
            What::Deliver {
                to,
                message,
                took,
                reaction,
                ..
            } => {
                let mark = match took {
                    Took::Known => "",
                    Took::Unknown => " [black box]",
                    Took::Manual => " [manual]",
                };
                format!("{t}    {} ← {message}{mark}: {reaction}", to.label)
            }
            What::Variable {
                name,
                old,
                value,
                by,
                ..
            } => format!("{t}  {} sets {name}: {old} → {value}", by.label),
            What::Input { text } => format!("{t}  input: {text}"),
            What::Lua { text } => format!("{t}  Lua: {text}"),
            What::Warning { text } => format!("{t}  warning: {text}"),
        }
    }
}

/// One message of a journal of the game.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct GameMessage {
    /// Game time, seconds, when known.
    pub time: Option<f64>,
    pub hash: u32,
    pub name: Option<String>,
    /// GUIDs of the receivers (32 hexadecimal digits in memory order).
    pub receivers: Vec<String>,
}

fn parse_hash(v: &serde_json::Value) -> Option<u32> {
    match v {
        serde_json::Value::Number(n) => n.as_u64().map(|n| n as u32),
        serde_json::Value::String(s) => {
            let t = s.trim().trim_start_matches("0x").trim_start_matches("0X");
            u32::from_str_radix(t, 16).ok()
        }
        _ => None,
    }
}

fn guid_text(v: &serde_json::Value) -> Option<String> {
    let s = match v {
        serde_json::Value::String(s) => s.as_str(),
        serde_json::Value::Object(o) => o.get("guid")?.as_str()?,
        _ => return None,
    };
    crate::live::proto::parse_guid(s).map(|g| crate::live::proto::guid_hex(&g))
}

fn message_of(o: &serde_json::Map<String, serde_json::Value>) -> Option<GameMessage> {
    let kind = ["kind", "type", "event"]
        .iter()
        .find_map(|k| o.get(*k).and_then(|v| v.as_str()))
        .unwrap_or("message");
    if !matches!(kind, "message" | "send" | "Message") {
        return None;
    }
    let name = ["message", "name"]
        .iter()
        .find_map(|k| o.get(*k).and_then(|v| v.as_str()))
        .map(str::to_owned);
    let hash = o
        .get("hash")
        .and_then(parse_hash)
        .or_else(|| name.as_deref().map(string_id))?;
    let time = o
        .get("sim_time_ms")
        .and_then(serde_json::Value::as_f64)
        .map(|ms| ms / 1000.0)
        .or_else(|| o.get("time").and_then(serde_json::Value::as_f64));
    let receivers = ["receivers", "to"]
        .iter()
        .find_map(|k| o.get(*k).and_then(|v| v.as_array()))
        .map(|a| a.iter().filter_map(guid_text).collect())
        .unwrap_or_default();
    Some(GameMessage {
        time,
        hash,
        name,
        receivers,
    })
}

/// Reads the messages of a journal in JSON: an array of events, an object
/// with an `events` (or `journal`) array, or one event per line. An event
/// is a message when its `kind` (or `type`, `event`) is `message` or
/// `send`, or when it has none; it names the message by `hash` (number or
/// hex) or `message`/`name`, its time by `sim_time_ms` or `time`
/// (seconds), its receivers by `receivers` (or `to`): GUID strings, braced
/// or 32 hex digits, or objects with `guid`. A journal of a run exported
/// with `--json` reads the same way.
pub fn parse_game_journal(text: &str) -> crate::Result<Vec<GameMessage>> {
    let events: Vec<serde_json::Value> = match serde_json::from_str::<serde_json::Value>(text) {
        Ok(serde_json::Value::Array(a)) => a,
        Ok(serde_json::Value::Object(o)) => {
            match ["events", "journal"]
                .iter()
                .find_map(|k| o.get(*k).and_then(|v| v.as_array()))
            {
                Some(a) => a.clone(),
                None => vec![serde_json::Value::Object(o)],
            }
        }
        Ok(_) => Vec::new(),
        Err(_) => text
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(serde_json::from_str)
            .collect::<std::result::Result<_, _>>()
            .map_err(|e| crate::Error::InvalidArgument(format!("journal: {e}")))?,
    };
    Ok(events
        .iter()
        .filter_map(|e| e.as_object().and_then(message_of))
        .collect())
}

/// A message both journals have.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Matched {
    pub message: String,
    /// Entry of the run.
    pub seq: u64,
    /// Index of the game's message.
    pub game: usize,
    pub run_time: f64,
    pub game_time: Option<f64>,
    /// Receivers the run has and the game does not, and the other way
    /// round (GUIDs; the game lists at most 8 receivers).
    pub only_run: Vec<String>,
    pub only_game: Vec<String>,
}

/// The comparison of a run with a journal of the game.
#[derive(Debug, Clone, PartialEq, Serialize, Default)]
pub struct Comparison {
    pub matched: Vec<Matched>,
    /// Messages of the run the game did not send: entry, name.
    pub only_run: Vec<(u64, String)>,
    /// Messages of the game the run did not send: index, name or hash.
    pub only_game: Vec<(usize, String)>,
    /// Messages of the game left out: not used in the scope.
    pub ignored: usize,
    /// Time of the game minus time of the run at the first common message.
    pub offset: Option<f64>,
}

impl Comparison {
    /// The largest difference of time between matched messages, after the
    /// offset.
    pub fn worst_drift(&self) -> f64 {
        let Some(offset) = self.offset else {
            return 0.0;
        };
        self.matched
            .iter()
            .filter_map(|m| m.game_time.map(|g| (g - offset - m.run_time).abs()))
            .fold(0.0, f64::max)
    }
}

/// Compares the messages a run sent with the messages of a journal of the
/// game. Only game messages whose hash the scope uses (`scope`) are
/// compared unless `scope` is empty. The two sequences are aligned by the
/// longest common subsequence of hashes (greedily when they are long).
pub fn compare(
    journal: &[Entry],
    game: &[GameMessage],
    scope: &std::collections::HashSet<u32>,
) -> Comparison {
    struct RunMsg<'a> {
        seq: u64,
        time: f64,
        hash: u32,
        name: &'a str,
        to: Vec<String>,
    }
    let run: Vec<RunMsg<'_>> = journal
        .iter()
        .filter_map(|e| match &e.what {
            What::Send {
                message, hash, to, ..
            } => Some(RunMsg {
                seq: e.seq,
                time: e.time,
                hash: u32::from_str_radix(hash, 16).unwrap_or_else(|_| string_id(message)),
                name: message,
                to: to.iter().filter_map(|w| w.guid.clone()).collect(),
            }),
            _ => None,
        })
        .collect();
    let mut out = Comparison::default();
    let kept: Vec<usize> = (0..game.len())
        .filter(|&i| scope.is_empty() || scope.contains(&game[i].hash))
        .collect();
    out.ignored = game.len() - kept.len();
    let pairs = align(
        &run.iter().map(|r| r.hash).collect::<Vec<_>>(),
        &kept.iter().map(|&i| game[i].hash).collect::<Vec<_>>(),
    );
    let mut used_run = vec![false; run.len()];
    let mut used_game = vec![false; kept.len()];
    for (a, b) in pairs {
        used_run[a] = true;
        used_game[b] = true;
        let r = &run[a];
        let g = &game[kept[b]];
        if out.offset.is_none() {
            out.offset = g.time.map(|t| t - r.time);
        }
        let only_run = if g.receivers.is_empty() {
            Vec::new()
        } else {
            r.to.iter()
                .filter(|x| !g.receivers.contains(x))
                .cloned()
                .collect()
        };
        let only_game = g
            .receivers
            .iter()
            .filter(|x| !r.to.contains(x))
            .cloned()
            .collect();
        out.matched.push(Matched {
            message: r.name.to_owned(),
            seq: r.seq,
            game: kept[b],
            run_time: r.time,
            game_time: g.time,
            only_run,
            only_game,
        });
    }
    for (i, r) in run.iter().enumerate() {
        if !used_run[i] {
            out.only_run.push((r.seq, r.name.to_owned()));
        }
    }
    let names: BTreeMap<u32, &str> = run.iter().map(|r| (r.hash, r.name)).collect();
    for (j, &i) in kept.iter().enumerate() {
        if !used_game[j] {
            let g = &game[i];
            let name = g
                .name
                .clone()
                .or_else(|| names.get(&g.hash).map(|s| (*s).to_owned()))
                .unwrap_or_else(|| format!("{:08x}", g.hash));
            out.only_game.push((i, name));
        }
    }
    out
}

/// Pairs of equal elements of `a` and `b` in order: the longest common
/// subsequence when `a.len() × b.len()` is small, otherwise a greedy match
/// within a window.
fn align(a: &[u32], b: &[u32]) -> Vec<(usize, usize)> {
    const LIMIT: usize = 16_000_000;
    if a.len().saturating_mul(b.len()) <= LIMIT {
        let (n, m) = (a.len(), b.len());
        let mut dp = vec![0u32; (n + 1) * (m + 1)];
        let at = |i: usize, j: usize| i * (m + 1) + j;
        for i in (0..n).rev() {
            for j in (0..m).rev() {
                dp[at(i, j)] = if a[i] == b[j] {
                    dp[at(i + 1, j + 1)] + 1
                } else {
                    dp[at(i + 1, j)].max(dp[at(i, j + 1)])
                };
            }
        }
        let mut out = Vec::new();
        let (mut i, mut j) = (0, 0);
        while i < n && j < m {
            if a[i] == b[j] {
                out.push((i, j));
                i += 1;
                j += 1;
            } else if dp[at(i + 1, j)] >= dp[at(i, j + 1)] {
                i += 1;
            } else {
                j += 1;
            }
        }
        out
    } else {
        const WINDOW: usize = 256;
        let mut out = Vec::new();
        let mut j0 = 0;
        for (i, x) in a.iter().enumerate() {
            if let Some(k) = b[j0..b.len().min(j0 + WINDOW)].iter().position(|y| y == x) {
                out.push((i, j0 + k));
                j0 += k + 1;
            }
        }
        out
    }
}

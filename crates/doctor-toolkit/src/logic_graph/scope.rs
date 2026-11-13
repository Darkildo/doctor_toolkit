//! What part of the game a graph shows, and reading its SimGroups.
//!
//! Missions live in directories of streams: `gf2/<city>/global/objectives/
//! <mission>/…` and `gf2/…/checkpoints/<mission>/…` (`cp_710_725_mission_7`,
//! `c67-strategygame-parta-killmitchell`). A checkpoint is a stream whose
//! name has `cp_<number>` in it.

use std::collections::{BTreeMap, HashMap};

use rayon::prelude::*;

use crate::game::{GameInstance, ResourceIndex, StreamFile};
use crate::level::{Location, TYPE_SIMGROUP};
use crate::resource::simgroup::SimGroup;
use crate::workspace::{ResourceKey, Workspace};
use crate::{Error, Result};

/// What part of the game to show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScopeSpec {
    /// One SimGroup.
    Group(Location),
    /// Every SimGroup of these streams.
    Streams(Vec<usize>),
    /// Every SimGroup of a mission directory (see [`missions`]).
    Mission(String),
    /// The streams whose name has `cp_<number>`.
    Checkpoint(u32),
    /// A stream with its ancestors in the tables of contents (what the game
    /// has loaded while the stream is).
    Level(usize),
    /// The whole game.
    Game,
}

impl ScopeSpec {
    /// Parses `group:<stream>#<resource>`, `stream:<stream>`,
    /// `mission:<name>`, `checkpoint:<number>`, `level:<stream>`, `game`;
    /// a bare text is a stream name or else a mission name.
    pub fn parse(game: &GameInstance, text: &str) -> Result<Self> {
        let stream = |name: &str| {
            game.find_stream(name.trim_end_matches(".str"))
                .ok_or_else(|| Error::InvalidArgument(format!("stream `{name}` not found")))
        };
        let (kind, arg) = text.split_once(':').unwrap_or(("", text));
        Ok(match kind {
            "group" => {
                let (s, r) = arg.rsplit_once('#').ok_or_else(|| {
                    Error::InvalidArgument("expected group:<stream>#<resource>".into())
                })?;
                let resource = r
                    .parse()
                    .map_err(|_| Error::InvalidArgument(format!("bad resource number `{r}`")))?;
                Self::Group(Location {
                    stream: stream(s)?,
                    resource,
                })
            }
            "stream" => Self::Streams(vec![stream(arg)?]),
            "mission" => Self::Mission(arg.to_owned()),
            "checkpoint" | "cp" => Self::Checkpoint(
                arg.trim_start_matches("cp_")
                    .parse()
                    .map_err(|_| Error::InvalidArgument(format!("bad checkpoint `{arg}`")))?,
            ),
            "level" => Self::Level(stream(arg)?),
            "" if text == "game" => Self::Game,
            "" => match stream(text) {
                Ok(s) => Self::Streams(vec![s]),
                Err(_) => Self::Mission(text.to_owned()),
            },
            _ => {
                return Err(Error::InvalidArgument(format!(
                    "unknown scope `{kind}`; use group:, stream:, mission:, checkpoint:, level: or game"
                )));
            }
        })
    }

    /// A short label for the scope.
    pub fn label(&self, game: &GameInstance) -> String {
        let name = |s: usize| {
            game.streams()
                .get(s)
                .map_or_else(|| format!("#{s}"), |f| f.stream_name())
        };
        match self {
            Self::Group(l) => format!("{} #{}", name(l.stream), l.resource),
            Self::Streams(s) if s.len() == 1 => name(s[0]),
            Self::Streams(s) => format!("{} streams", s.len()),
            Self::Mission(m) => format!("mission {m}"),
            Self::Checkpoint(c) => format!("checkpoint {c}"),
            Self::Level(s) => format!("level {}", name(*s)),
            Self::Game => "whole game".into(),
        }
    }
}

/// A mission: a directory of streams under `objectives` or `checkpoints`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Mission {
    /// Directory name, the same in every city.
    pub name: String,
    /// Streams in it (and in its subdirectories).
    pub streams: Vec<usize>,
}

/// Mission directory of a stream name, if it is in one.
pub fn mission_of(stream_name: &str) -> Option<&str> {
    let parts: Vec<&str> = stream_name.split(['/', '\\']).collect();
    let at = parts.iter().position(|p| {
        p.eq_ignore_ascii_case("objectives") || p.eq_ignore_ascii_case("checkpoints")
    })?;
    // The directory after `objectives`, if the stream is inside it.
    (at + 2 < parts.len()).then(|| parts[at + 1])
}

/// Every mission of the game, by name.
pub fn missions(game: &GameInstance) -> Vec<Mission> {
    let mut by_name: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (i, f) in game.streams().iter().enumerate() {
        if let Some(m) = mission_of(&f.stream_name()) {
            by_name.entry(m.to_ascii_lowercase()).or_default().push(i);
        }
    }
    by_name
        .into_iter()
        .map(|(name, streams)| Mission { name, streams })
        .collect()
}

/// The streams of a scope.
pub fn streams(game: &GameInstance, spec: &ScopeSpec) -> Result<Vec<usize>> {
    Ok(match spec {
        ScopeSpec::Group(l) => vec![l.stream],
        ScopeSpec::Streams(s) => s.clone(),
        ScopeSpec::Mission(name) => {
            let all = missions(game);
            let want = name.to_ascii_lowercase();
            let exact: Vec<&Mission> = all.iter().filter(|m| m.name == want).collect();
            let found: Vec<&Mission> = if exact.is_empty() {
                all.iter().filter(|m| m.name.contains(&want)).collect()
            } else {
                exact
            };
            match found.as_slice() {
                [m] => m.streams.clone(),
                [] => {
                    return Err(Error::InvalidArgument(format!(
                        "no mission matches `{name}`"
                    )));
                }
                many => {
                    let names: Vec<&str> = many.iter().take(8).map(|m| m.name.as_str()).collect();
                    return Err(Error::InvalidArgument(format!(
                        "`{name}` matches {} missions: {}",
                        many.len(),
                        names.join(", ")
                    )));
                }
            }
        }
        ScopeSpec::Checkpoint(n) => {
            let tag = format!("cp_{n:03}");
            let s: Vec<usize> = game
                .streams()
                .iter()
                .enumerate()
                .filter(|(_, f)| {
                    let name = f.stream_name().to_ascii_lowercase();
                    name.match_indices(&tag).any(|(at, _)| {
                        !name[at + tag.len()..].starts_with(|c: char| c.is_ascii_digit())
                    })
                })
                .map(|(i, _)| i)
                .collect();
            if s.is_empty() {
                return Err(Error::InvalidArgument(format!(
                    "no stream of checkpoint {n}"
                )));
            }
            s
        }
        ScopeSpec::Level(s) => {
            let parents = game.stream_parents()?;
            crate::level::Scope::new(game, &parents, &[*s]).streams
        }
        ScopeSpec::Game => (0..game.streams().len()).collect(),
    })
}

/// SimGroups of a scope.
pub fn groups(
    game: &GameInstance,
    index: &ResourceIndex,
    spec: &ScopeSpec,
) -> Result<Vec<Location>> {
    if let ScopeSpec::Group(l) = spec {
        return Ok(vec![*l]);
    }
    let streams = streams(game, spec)?;
    let mut by_stream: HashMap<usize, Vec<usize>> = HashMap::new();
    for e in &index.entries {
        if e.header.type_id == TYPE_SIMGROUP {
            by_stream
                .entry(e.stream as usize)
                .or_default()
                .push(e.resource as usize);
        }
    }
    let mut out = Vec::new();
    for s in streams {
        for &r in by_stream.get(&s).into_iter().flatten() {
            out.push(Location {
                stream: s,
                resource: r,
            });
        }
    }
    Ok(out)
}

/// A SimGroup read for a graph.
pub struct LoadedGroup {
    pub location: Location,
    pub stream: String,
    pub group: SimGroup,
}

/// Reads and parses SimGroups, the workspace version when it has one;
/// groups that fail to read are returned as errors.
pub fn load(
    game: &GameInstance,
    ws: Option<&Workspace>,
    locations: &[Location],
) -> (Vec<LoadedGroup>, Vec<(Location, Error)>) {
    let mut by_stream: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for l in locations {
        by_stream.entry(l.stream).or_default().push(l.resource);
    }
    let results: Vec<Vec<(Location, Result<LoadedGroup>)>> = by_stream
        .into_par_iter()
        .map(|(s, resources)| {
            let file = &game.streams()[s];
            let name = file.stream_name();
            let at = |r: usize| Location {
                stream: s,
                resource: r,
            };
            let stream = match StreamFile::open(&file.path) {
                Ok(stream) => stream,
                Err(e) => {
                    let text = e.to_string();
                    return resources
                        .iter()
                        .map(|&r| (at(r), Err(Error::InvalidArgument(text.clone()))))
                        .collect();
                }
            };
            let read = |r: usize| -> Result<LoadedGroup> {
                let header = stream
                    .layout()
                    .resources
                    .get(r)
                    .ok_or_else(|| Error::InvalidArgument(format!("no resource {r}")))?
                    .header;
                let key = ResourceKey::new(&name, &header);
                let data = match ws.map(|ws| ws.data(&key)).transpose()?.flatten() {
                    Some(d) => d,
                    None => stream.resource_data(r)?,
                };
                Ok(LoadedGroup {
                    location: at(r),
                    stream: name.clone(),
                    group: SimGroup::parse(&data)?,
                })
            };
            resources.into_iter().map(|r| (at(r), read(r))).collect()
        })
        .collect();
    let mut ok = Vec::new();
    let mut errors = Vec::new();
    for (location, r) in results.into_iter().flatten() {
        match r {
            Ok(g) => ok.push(g),
            Err(e) => errors.push((location, e)),
        }
    }
    (ok, errors)
}

/// Reads the SimGroups of a scope and builds its graph.
pub fn build(
    game: &GameInstance,
    index: &ResourceIndex,
    ws: Option<&Workspace>,
    spec: &ScopeSpec,
    options: super::BuildOptions,
) -> Result<(super::LogicGraph, Vec<(Location, Error)>)> {
    let locations = groups(game, index, spec)?;
    let (loaded, errors) = load(game, ws, &locations);
    let mut builder = super::GraphBuilder::new(super::LogicSchema::gf2());
    for g in &loaded {
        builder.add_group(g.location, &g.stream, &g.group);
    }
    Ok((builder.build(options), errors))
}

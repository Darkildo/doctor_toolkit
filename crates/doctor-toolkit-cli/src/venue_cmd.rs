//! `venue`: the venues of a city (`bricknmortarres`). What it does is in
//! [`doctor_toolkit::level::venues`].

use std::io::Write as _;
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Args, Subcommand};

use doctor_toolkit::game::GameInstance;
use doctor_toolkit::level::venues::{self, NewVenue, VenueChange};
use doctor_toolkit::resource::bricknmortar::Venue;
use doctor_toolkit::workspace::Workspace;

use crate::workspace_cmd::{self, EditMode};

macro_rules! out {
    ($($arg:tt)*) => {
        if writeln!(std::io::stdout(), $($arg)*).is_err() {
            std::process::exit(0);
        }
    };
}

#[derive(Subcommand)]
pub enum VenueCommand {
    /// List the venues of a city: number, owner, type, position
    List(VenueList),
    /// Add a venue made after a venue of the same city (recorded in the
    /// workspace)
    ///
    /// The game finds the venue of a `Building` object by its number and
    /// crashes when there is none, so a building with a new number needs
    /// this record first. Then copy a `Building` object (`level add`), set
    /// `Building.0` to the number and `Building.5` to the GUID printed
    /// here, and add the string `building_name_NNN` (`text-add`) for its
    /// name on screen.
    Add(VenueAdd),
    /// Change the owner, type, flags or position of a venue (recorded in the
    /// workspace)
    Set(VenueSet),
    /// Remove a venue (recorded in the workspace); a building with its
    /// number crashes the game afterwards
    Remove(VenueRemove),
}

#[derive(Args)]
pub struct VenueList {
    game_dir: PathBuf,
    /// Root stream of the city, e.g. `gf2/queens`
    level: String,
    /// Print the venues as JSON
    #[arg(long)]
    json: bool,
    /// Workspace directory [default: <GAME_DIR>/_doctor_toolkit/workspace]
    #[arg(long, value_name = "DIR")]
    workspace_dir: Option<PathBuf>,
}

#[derive(Args)]
pub struct VenueAdd {
    game_dir: PathBuf,
    /// Root stream of the city, e.g. `gf2/queens`
    level: String,
    /// Number of the new venue
    #[arg(long)]
    id: i32,
    /// Number of the venue to copy: type, flags, effects, owner
    #[arg(long)]
    like: i32,
    /// Owning family: a name (`rosato_carmine`, `corleone`), its hash as 8
    /// hexadecimal digits, or `none` [default: that of --like]
    #[arg(long)]
    owner: Option<String>,
    /// Position of the building, `X,Y,Z` [default: that of --like]
    #[arg(long, value_name = "X,Y,Z", allow_hyphen_values = true)]
    position: Option<String>,
    /// GUID of the venue, 32 hexadecimal digits [default: a new one]
    #[arg(long)]
    guid: Option<String>,
    #[command(flatten)]
    mode: EditMode,
}

#[derive(Args)]
pub struct VenueSet {
    game_dir: PathBuf,
    level: String,
    /// Number of the venue
    id: i32,
    /// Owning family: a name, 8 hexadecimal digits or `none`
    #[arg(long)]
    owner: Option<String>,
    /// Type 0..9
    #[arg(long = "type")]
    kind: Option<i32>,
    /// Flags (decimal, or hexadecimal with 0x)
    #[arg(long)]
    flags: Option<String>,
    /// Position of the building, `X,Y,Z`
    #[arg(long, value_name = "X,Y,Z", allow_hyphen_values = true)]
    position: Option<String>,
    #[command(flatten)]
    mode: EditMode,
}

#[derive(Args)]
pub struct VenueRemove {
    game_dir: PathBuf,
    level: String,
    /// Number of the venue
    id: i32,
    #[command(flatten)]
    mode: EditMode,
}

pub fn venue(command: VenueCommand) -> Result<()> {
    match command {
        VenueCommand::List(args) => list(args),
        VenueCommand::Add(args) => add(args),
        VenueCommand::Set(args) => set(args),
        VenueCommand::Remove(args) => remove(args),
    }
}

fn parse_point(text: &str) -> Result<[f32; 3]> {
    let parts: Vec<f32> = text
        .split(',')
        .map(|p| p.trim().parse::<f32>())
        .collect::<std::result::Result<_, _>>()
        .with_context(|| format!("`{text}` is not X,Y,Z"))?;
    <[f32; 3]>::try_from(parts).map_err(|_| anyhow::anyhow!("`{text}` is not X,Y,Z"))
}

fn parse_flags(text: &str) -> Result<u32> {
    let t = text.trim();
    match t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
        Some(hex) => u32::from_str_radix(hex, 16),
        None => t.parse(),
    }
    .with_context(|| format!("`{text}` is not a number"))
}

/// Family names by their owner hash, from the `family` value list.
fn family_name(hash: u32) -> String {
    if hash == 0 {
        return "-".into();
    }
    doctor_toolkit::game_types::enum_def("family")
        .and_then(|e| e.values.iter().find(|v| v.hash == Some(hash)))
        .and_then(|v| v.name)
        .map_or_else(|| format!("{hash:08x}"), str::to_owned)
}

fn line(v: &Venue) -> String {
    format!(
        "{:5}  type {}  owner {:<16} flags {:#06x}  at {:.1},{:.1},{:.1}  guid {}",
        v.id,
        v.kind,
        family_name(v.owner),
        v.flags,
        v.position[0],
        v.position[1],
        v.position[2],
        v.guid
    )
}

fn list(args: VenueList) -> Result<()> {
    let game = GameInstance::open(&args.game_dir)?;
    let ws = Workspace::for_game(&game, args.workspace_dir.as_deref())?;
    let city = venues::city_venues(&game, &ws, &args.level)?;
    if args.json {
        out!("{}", serde_json::to_string_pretty(&city.table)?);
        return Ok(());
    }
    out!(
        "#{} bricknmortarres of {}: {} venue(s)",
        city.resource,
        args.level,
        city.table.venues.len()
    );
    let mut sorted: Vec<&Venue> = city.table.venues.iter().collect();
    sorted.sort_by_key(|v| v.id);
    for v in sorted {
        out!("{}", line(v));
    }
    Ok(())
}

fn add(args: VenueAdd) -> Result<()> {
    let game = GameInstance::open(&args.game_dir)?;
    let mut ws = workspace_cmd::open_for_edit(&game, &args.mode)?;
    let new = NewVenue {
        id: args.id,
        like: args.like,
        owner: args.owner.as_deref().map(venues::family_hash),
        position: args.position.as_deref().map(parse_point).transpose()?,
        guid: args.guid.clone(),
    };
    let venue = venues::add_venue(&game, &mut ws, &args.level, &new)?;
    out!("added {}", line(&venue));
    out!(
        "next: a Building with Building.0={} and Building.5={}, and the string building_name_{:03}",
        venue.id,
        venue.guid,
        venue.id
    );
    let level = doctor_toolkit::workspace::key::normalize_stream(&args.level);
    workspace_cmd::finish_edit(&game, &mut ws, &args.mode, vec![level])
}

fn set(args: VenueSet) -> Result<()> {
    let game = GameInstance::open(&args.game_dir)?;
    let mut ws = workspace_cmd::open_for_edit(&game, &args.mode)?;
    let change = VenueChange {
        owner: args.owner.as_deref().map(venues::family_hash),
        kind: args.kind,
        flags: args.flags.as_deref().map(parse_flags).transpose()?,
        position: args.position.as_deref().map(parse_point).transpose()?,
    };
    let venue = venues::change_venue(&game, &mut ws, &args.level, args.id, &change)?;
    out!("changed {}", line(&venue));
    let level = doctor_toolkit::workspace::key::normalize_stream(&args.level);
    workspace_cmd::finish_edit(&game, &mut ws, &args.mode, vec![level])
}

fn remove(args: VenueRemove) -> Result<()> {
    let game = GameInstance::open(&args.game_dir)?;
    let mut ws = workspace_cmd::open_for_edit(&game, &args.mode)?;
    let venue = venues::remove_venue(&game, &mut ws, &args.level, args.id)?;
    out!("removed {}", line(&venue));
    let level = doctor_toolkit::workspace::key::normalize_stream(&args.level);
    workspace_cmd::finish_edit(&game, &mut ws, &args.mode, vec![level])
}

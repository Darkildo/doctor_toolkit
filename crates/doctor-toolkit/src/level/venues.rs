//! The venues of a city (`bricknmortarres` in the root stream of the level,
//! [`crate::resource::bricknmortar`]): list them, add a new one made after a
//! venue of the game, change and remove them.
//!
//! A new venue needs its record here before any `Building` object carries
//! its number: the game finds the venue of a building by number and crashes
//! when there is none. The building itself is a copy of a `Building` object
//! with `Building.0` = the new number and `Building.5` = the GUID of the new
//! record; its name on screen is the string `building_name_NNN`.

use crate::game::GameInstance;
use crate::hash::sdbm;
use crate::resource::bricknmortar::{TYPE_BRICKNMORTARRES, Venue, VenueTable};
use crate::resource::neoguid::{self, MachineName};
use crate::resource::simgroup::Guid;
use crate::workspace::Workspace;
use crate::{Error, Result};

/// The venue table of a level root stream as the workspace has it.
#[derive(Debug, Clone)]
pub struct CityVenues {
    /// Index of the stream.
    pub stream: usize,
    /// Index of the resource in the stream.
    pub resource: usize,
    pub table: VenueTable,
}

/// A venue to add: a copy of venue `like` of the same city.
#[derive(Debug, Clone, PartialEq)]
pub struct NewVenue {
    /// Number of the new venue; no venue of the city may have it.
    pub id: i32,
    /// Number of the venue to copy.
    pub like: i32,
    /// sdbm of the lower-case name of the owning family [default: that of
    /// `like`].
    pub owner: Option<u32>,
    /// Position of the building [default: that of `like`]; the other two
    /// points move with it.
    pub position: Option<[f32; 3]>,
    /// GUID as 32 hexadecimal digits [default: a new one, derived from the
    /// level and the number].
    pub guid: Option<String>,
}

/// Changes to one venue; `None` keeps the value.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct VenueChange {
    pub owner: Option<u32>,
    pub kind: Option<i32>,
    pub flags: Option<u32>,
    pub position: Option<[f32; 3]>,
}

/// The owner hash of a family name (`rosato_carmine`), or 8 hexadecimal
/// digits as they are; `none` is 0.
pub fn family_hash(text: &str) -> u32 {
    let t = text.trim();
    if t.eq_ignore_ascii_case("none") {
        return 0;
    }
    if t.len() == 8
        && let Ok(h) = u32::from_str_radix(t, 16)
    {
        return h;
    }
    sdbm(t.to_ascii_lowercase().as_bytes())
}

/// The venue table of level root stream `level` (`gf2/queens`).
pub fn city_venues(game: &GameInstance, ws: &Workspace, level: &str) -> Result<CityVenues> {
    let stream = game
        .find_stream(level)
        .ok_or_else(|| Error::InvalidArgument(format!("no stream {level}")))?;
    let opened = game.open_stream(stream)?;
    let resource = opened
        .layout()
        .resources
        .iter()
        .position(|e| e.header.type_id == TYPE_BRICKNMORTARRES)
        .ok_or_else(|| {
            Error::InvalidArgument(format!("{level} has no venue table (`bricknmortarres`)"))
        })?;
    let data = ws.current_data(game, stream, resource)?;
    Ok(CityVenues {
        stream,
        resource,
        table: VenueTable::parse(&data)?,
    })
}

/// The venue tables of every level as the workspace has them, with the name
/// of their stream: the root streams of the cities (`gf2/queens`,
/// `gf2/miami`, `gf2/cuba`) and of the multiplayer maps.
///
/// The game keeps the venues of every table it has loaded in **one** table
/// sorted by number (`0x006B1520` → `0x006B1280`): a number that two tables
/// share makes the insertion fail and the game writes through a null pointer
/// when the second one loads (`0x006B153C`, seen on Windows with venue 230 of
/// a mod and venue 230 of Havana). Numbers are unique across all of them.
pub fn all_venue_tables(game: &GameInstance, ws: &Workspace) -> Result<Vec<(String, CityVenues)>> {
    let mut out = Vec::new();
    for (stream, file) in game.streams().iter().enumerate() {
        // tables are in the root stream of a level: one or two path parts
        if file.rel_path.matches('/').count() > 1 {
            continue;
        }
        let opened = game.open_stream(stream)?;
        let Some(resource) = opened
            .layout()
            .resources
            .iter()
            .position(|e| e.header.type_id == TYPE_BRICKNMORTARRES)
        else {
            continue;
        };
        let data = ws.current_data(game, stream, resource)?;
        out.push((
            file.stream_name(),
            CityVenues {
                stream,
                resource,
                table: VenueTable::parse(&data)?,
            },
        ));
    }
    Ok(out)
}

/// The level whose table has venue `id`, if any.
pub fn venue_owner_level(game: &GameInstance, ws: &Workspace, id: i32) -> Result<Option<String>> {
    Ok(all_venue_tables(game, ws)?
        .into_iter()
        .find(|(_, city)| city.table.find(id).is_some())
        .map(|(name, _)| name))
}

fn save(game: &GameInstance, ws: &mut Workspace, city: &CityVenues, label: &str) -> Result<()> {
    let problems = city.table.validate();
    if let Some(first) = problems.first() {
        return Err(Error::InvalidArgument(first.clone()));
    }
    ws.set_resource(
        game,
        city.stream,
        city.resource,
        city.table.to_bytes()?,
        label,
    )?;
    Ok(())
}

fn moved(point: [f32; 3], delta: [f32; 3]) -> [f32; 3] {
    [
        point[0] + delta[0],
        point[1] + delta[1],
        point[2] + delta[2],
    ]
}

/// Adds a venue to the city of `level` (recorded in the workspace) and
/// returns it.
pub fn add_venue(
    game: &GameInstance,
    ws: &mut Workspace,
    level: &str,
    new: &NewVenue,
) -> Result<Venue> {
    let mut city = city_venues(game, ws, level)?;
    if let Some(other) = venue_owner_level(game, ws, new.id)? {
        return Err(Error::InvalidArgument(format!(
            "venue {} is already in {other}: the numbers of all cities share one table in the game, \
             and a repeated one crashes it when the second city loads",
            new.id
        )));
    }
    let mut venue = city
        .table
        .find(new.like)
        .cloned()
        .ok_or_else(|| Error::InvalidArgument(format!("{level} has no venue {}", new.like)))?;
    venue.id = new.id;
    if let Some(owner) = new.owner {
        venue.owner = owner;
    }
    if let Some(at) = new.position {
        let delta = [
            at[0] - venue.position[0],
            at[1] - venue.position[1],
            at[2] - venue.position[2],
        ];
        venue.position = at;
        venue.point_b = moved(venue.point_b, delta);
        venue.point_c = moved(venue.point_c, delta);
    }
    venue.guid = match &new.guid {
        Some(text) => {
            let bytes = crate::create::parse_guid(text)?;
            bytes.iter().map(|b| format!("{b:02x}")).collect()
        }
        None => {
            let taken: Vec<String> = city.table.venues.iter().map(|v| v.guid.clone()).collect();
            let guid = neoguid::new_guid(
                &MachineName::TOOLKIT,
                &format!("venue/{level}/{}", new.id),
                |g: &Guid| {
                    let text: String = g.0.iter().map(|b| format!("{b:02x}")).collect();
                    taken.contains(&text)
                },
            );
            guid.0.iter().map(|b| format!("{b:02x}")).collect()
        }
    };
    city.table.venues.push(venue.clone());
    save(
        game,
        ws,
        &city,
        &format!("Add venue {} to {level} after venue {}", new.id, new.like),
    )?;
    Ok(venue)
}

/// Changes venue `id` of the city of `level` (recorded in the workspace).
pub fn change_venue(
    game: &GameInstance,
    ws: &mut Workspace,
    level: &str,
    id: i32,
    change: &VenueChange,
) -> Result<Venue> {
    let mut city = city_venues(game, ws, level)?;
    let venue = city
        .table
        .find_mut(id)
        .ok_or_else(|| Error::InvalidArgument(format!("{level} has no venue {id}")))?;
    if let Some(owner) = change.owner {
        venue.owner = owner;
    }
    if let Some(kind) = change.kind {
        venue.kind = kind;
    }
    if let Some(flags) = change.flags {
        venue.flags = flags;
    }
    if let Some(at) = change.position {
        let delta = [
            at[0] - venue.position[0],
            at[1] - venue.position[1],
            at[2] - venue.position[2],
        ];
        venue.position = at;
        venue.point_b = moved(venue.point_b, delta);
        venue.point_c = moved(venue.point_c, delta);
    }
    let venue = venue.clone();
    save(game, ws, &city, &format!("Change venue {id} of {level}"))?;
    Ok(venue)
}

/// Removes venue `id` from the city of `level` (recorded in the workspace).
/// A `Building` object with its number crashes the game afterwards.
pub fn remove_venue(
    game: &GameInstance,
    ws: &mut Workspace,
    level: &str,
    id: i32,
) -> Result<Venue> {
    let mut city = city_venues(game, ws, level)?;
    let index = city
        .table
        .venues
        .iter()
        .position(|v| v.id == id)
        .ok_or_else(|| Error::InvalidArgument(format!("{level} has no venue {id}")))?;
    let venue = city.table.venues.remove(index);
    save(game, ws, &city, &format!("Remove venue {id} from {level}"))?;
    Ok(venue)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn family_names_and_hashes() {
        assert_eq!(family_hash("rosato_carmine"), 0x529e27ee);
        assert_eq!(family_hash("Rosato_Carmine"), 0x529e27ee);
        assert_eq!(family_hash("corleone"), 0x0637b907);
        assert_eq!(family_hash("529e27ee"), 0x529e27ee);
        assert_eq!(family_hash("none"), 0);
    }
}

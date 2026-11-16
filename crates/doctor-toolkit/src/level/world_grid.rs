//! The world grid of a level: which streams the game loads by the position
//! of the player, and adding a stream of the workspace to them
//! (`docs/formats/wgi.md`, `docs/analysis/new-resources.md` §3.5).
//!
//! The root stream of a level (`gf2/cuba`) holds one `wgi` resource per
//! `WorldGraphPartition` object. A cell added to the grid of the high level
//! of detail makes the game load the stream within about 180 m of the
//! centre of the cell and unload it farther away, as it does with its own
//! zones.

use crate::game::GameInstance;
use crate::resource::wgi::{self, NewCell, WorldGrid};
use crate::workspace::Workspace;
use crate::{Error, Result};

/// One `wgi` resource of a level root stream.
#[derive(Debug, Clone)]
pub struct LevelGrid {
    /// Index of the resource in the stream.
    pub resource: usize,
    /// Resource id.
    pub id: u32,
    pub grid: WorldGrid,
}

/// A stream to load by the position of the player.
#[derive(Debug, Clone, PartialEq)]
pub struct ZoneCell {
    /// Name of the stream, e.g. `gf2/doctor/island`: a stream the workspace
    /// creates, or any stream when `guid` is given.
    pub stream: String,
    /// GUID of the stream as 32 hexadecimal digits [default: the GUID the
    /// workspace gave the new stream].
    pub guid: Option<String>,
    pub box_min: [f32; 3],
    pub box_max: [f32; 3],
    /// Bit of [`wgi::lod`]: the grid whose world cells have this mask gets
    /// the cell.
    pub lod_mask: u32,
    pub kind: u8,
    pub flags: u8,
    /// Name for `ZoneStreamEnable` objects.
    pub name: Option<String>,
}

/// What [`add_zone_cell`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ZoneCellAdded {
    /// Index of the `wgi` resource in the level stream.
    pub resource: usize,
    pub cell: usize,
    /// Table-of-contents hash of the stream of the cell.
    pub hash: u32,
    /// The stream the cell lists as needed, when the grid knows it.
    pub needs: Option<String>,
}

fn level_index(game: &GameInstance, level: &str) -> Result<usize> {
    game.find_stream(level)
        .ok_or_else(|| Error::InvalidArgument(format!("no stream {level}")))
}

/// The world grids of level root stream `level` as the workspace has them.
pub fn level_grids(game: &GameInstance, ws: &Workspace, level: &str) -> Result<Vec<LevelGrid>> {
    let stream_index = level_index(game, level)?;
    let stream = game.open_stream(stream_index)?;
    let mut grids = Vec::new();
    for (resource, entry) in stream.layout().resources.iter().enumerate() {
        if entry.header.type_id != wgi::TYPE_WGI {
            continue;
        }
        let data = ws.current_data(game, stream_index, resource)?;
        grids.push(LevelGrid {
            resource,
            id: entry.header.ids[0],
            grid: WorldGrid::parse(&data)?,
        });
    }
    Ok(grids)
}

/// Adds a cell for `cell.stream` to the grid of `level` whose world cells
/// have the level-of-detail mask `cell.lod_mask`, as one undoable operation.
pub fn add_zone_cell(
    game: &GameInstance,
    ws: &mut Workspace,
    level: &str,
    cell: &ZoneCell,
) -> Result<ZoneCellAdded> {
    let name = crate::workspace::key::normalize_stream(&cell.stream);
    let created = ws.new_stream(&name).cloned();
    let guid = match (&cell.guid, &created) {
        (Some(text), _) => crate::create::parse_guid(text)?,
        (None, Some(stream)) => crate::create::parse_guid(&stream.guid)?,
        (None, None) => {
            return Err(Error::InvalidArgument(format!(
                "{name} is not a stream of the workspace: give its GUID"
            )));
        }
    };
    let hash = crate::toc::guid_hash(&guid);
    if let Some(known) = game.stream_hash(&name)?
        && known != hash
    {
        return Err(Error::InvalidArgument(format!(
            "the GUID folds to {hash:08x}, the tables of contents list {name} as {known:08x}"
        )));
    }
    let stream_index = level_index(game, level)?;
    let mut grids = level_grids(game, ws, level)?;
    if let Some(grid) = grids.iter().find(|g| g.grid.find_cell(hash).is_some()) {
        return Err(Error::InvalidArgument(format!(
            "world grid {:08x} of {level} already has a cell of {name}",
            grid.id
        )));
    }
    let target = grids
        .iter_mut()
        .find(|g| g.grid.main_lod_mask() == Some(cell.lod_mask))
        .ok_or_else(|| {
            Error::InvalidArgument(format!(
                "{level} has no world grid with the level-of-detail mask {:#x}",
                cell.lod_mask
            ))
        })?;
    // The stream the cell needs: the parent of a new stream, when the grid
    // lists it.
    let mut needs = None;
    if let Some(stream) = &created
        && let Some(parent) = game.stream_hash(&stream.parent)?
        && target.grid.find_stream(parent).is_some()
    {
        needs = Some((parent, stream.parent.clone()));
    }
    let index = target.grid.add_cell(&NewCell {
        guid,
        box_min: cell.box_min,
        box_max: cell.box_max,
        kind: cell.kind,
        flags: cell.flags,
        lod_mask: cell.lod_mask,
        name: cell.name.clone(),
        needs: needs.as_ref().map(|(hash, _)| *hash),
    })?;
    let resource = target.resource;
    ws.set_resource(
        game,
        stream_index,
        resource,
        target.grid.to_bytes()?,
        &format!("Add {name} to the world grid of {level}"),
    )?;
    Ok(ZoneCellAdded {
        resource,
        cell: index,
        hash,
        needs: needs.map(|(_, name)| name),
    })
}

/// Removes the cell of the stream with table-of-contents hash `hash` from
/// the grids of `level`. Returns whether there was one.
pub fn remove_zone_cell(
    game: &GameInstance,
    ws: &mut Workspace,
    level: &str,
    hash: u32,
) -> Result<bool> {
    let stream_index = level_index(game, level)?;
    for mut grid in level_grids(game, ws, level)? {
        let Some(cell) = grid.grid.find_cell(hash) else {
            continue;
        };
        grid.grid.remove_cell(cell)?;
        // Drop pool words nothing points at any more (a word added for the
        // cell is the last one).
        while let Some(last) = grid.grid.pool.len().checked_sub(1) {
            let used =
                |l: &Option<wgi::PoolList>| l.is_some_and(|l| (l.start + l.count) as usize > last);
            if grid.grid.streams.iter().any(|s| used(&s.needs))
                || grid.grid.cells.iter().any(|c| used(&c.needs))
            {
                break;
            }
            grid.grid.pool.pop();
        }
        ws.set_resource(
            game,
            stream_index,
            grid.resource,
            grid.grid.to_bytes()?,
            &format!("Remove {hash:08x} from the world grid of {level}"),
        )?;
        return Ok(true);
    }
    Ok(false)
}

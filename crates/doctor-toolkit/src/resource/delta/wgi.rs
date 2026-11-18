//! Deltas of world grids: cells added and removed, a cell addressed by the
//! GUID of its stream.
//!
//! ```json
//! {"format":1,"kind":"wgi","ops":[
//!   {"op":"remove","stream":"<GUID>"},
//!   {"op":"add","stream":"<GUID>","centre":[10.0,4.5,10.0],"radius":15.0,
//!    "box_min":[0.0,-1.0,0.0,1.0],"box_max":[20.0,10.0,20.0,1.0],
//!    "kind":0,"flags":0,"lod_mask":1,"needs":["<GUID of the parent stream>"]}
//! ]}
//! ```
//!
//! Two mods that each add a zone to one city change the same `wgi` of the
//! root stream of the city; with deltas both cells get in. A new cell is in
//! a ring of its own, and the streams it needs are named by GUID: their
//! places in the stream table are not the same in every version of a grid.
//! A changed stream table or a changed cell keeps the grid whole.

use serde::{Deserialize, Serialize};

use super::{Applied, Delta, FORMAT, Float, Hex, Kind, Report, check_head, checked};
use crate::resource::simgroup::{Guid, Hash32};
use crate::resource::wgi::{Cell, NO_STREAM, WorldGrid};
use crate::{Error, Result};

/// A delta of a world grid.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GridDelta {
    pub format: u32,
    pub kind: Kind,
    pub ops: Vec<Op>,
}

fn is_zero(value: &Float) -> bool {
    value.0.to_bits() == 0
}

fn is_nil(value: &Hash32) -> bool {
    value.0 == 0
}

fn is_no_stream(value: &Hash32) -> bool {
    value.0 == NO_STREAM
}

fn no_stream() -> Hash32 {
    Hash32(NO_STREAM)
}

fn is_zero_word(value: &u32) -> bool {
    *value == 0
}

fn zero() -> Float {
    Float(0.0)
}

/// One operation on a world grid.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum Op {
    /// Removes the cell of a stream and the pool words only it used at the
    /// end of the pool.
    Remove { stream: Guid },
    /// Appends a cell, in a ring of its own. The fields are those of
    /// [`Cell`]; the hash of the stream is the fold of its GUID.
    Add {
        stream: Guid,
        centre: [Float; 3],
        radius: Float,
        box_min: [Float; 4],
        box_max: [Float; 4],
        kind: u8,
        flags: u8,
        lod_mask: u32,
        #[serde(default = "zero", skip_serializing_if = "is_zero")]
        min_distance: Float,
        #[serde(default = "zero", skip_serializing_if = "is_zero")]
        max_distance: Float,
        #[serde(default, skip_serializing_if = "is_nil")]
        name_hash: Hash32,
        #[serde(default = "no_stream", skip_serializing_if = "is_no_stream")]
        linked_stream: Hash32,
        /// GUIDs of the streams the cell needs; each must be in the stream
        /// table of the grid.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        needs: Vec<Guid>,
        /// The three fields the game does not read, when they are not zero.
        #[serde(default, skip_serializing_if = "Hex::is_zero")]
        unused_24: Hex,
        #[serde(default, skip_serializing_if = "is_zero_word")]
        reserved_64: u32,
        #[serde(default, skip_serializing_if = "Hex::is_zero")]
        unused_74: Hex,
    },
}

impl Op {
    /// What the operation addresses, as the notes of [`apply`] name it.
    pub fn target(&self) -> String {
        match self {
            Self::Remove { stream } | Self::Add { stream, .. } => {
                format!("cell of stream {stream}")
            }
        }
    }
}

/// The delta that turns `base` into `mine`, or `None` when the stream table
/// or a cell that stays changed, or the delta does not give `mine` back
/// byte for byte.
pub fn diff(base: &[u8], mine: &[u8]) -> Result<Option<GridDelta>> {
    let old = WorldGrid::parse(base)?;
    let new = WorldGrid::parse(mine)?;
    if old.streams != new.streams {
        return Ok(None);
    }
    let has = |grid: &WorldGrid, guid: &Guid| grid.cells.iter().any(|c| c.guid == *guid);
    let mut ops = Vec::new();
    for cell in old.cells.iter().filter(|c| !has(&new, &c.guid)) {
        ops.push(Op::Remove { stream: cell.guid });
    }
    for cell in new.cells.iter().filter(|c| !has(&old, &c.guid)) {
        ops.push(Op::Add {
            stream: cell.guid,
            centre: cell.centre.map(Float),
            radius: Float(cell.radius),
            box_min: cell.box_min.map(Float),
            box_max: cell.box_max.map(Float),
            kind: cell.kind,
            flags: cell.flags,
            lod_mask: cell.lod_mask,
            min_distance: Float(cell.min_distance),
            max_distance: Float(cell.max_distance),
            name_hash: cell.name_hash,
            linked_stream: cell.linked_stream,
            needs: new
                .list(&cell.needs)
                .iter()
                .map(|&s| new.streams[s as usize].guid)
                .collect(),
            unused_24: Hex(cell.unused_24.to_vec()),
            reserved_64: cell.reserved_64,
            unused_74: Hex(cell.unused_74.to_vec()),
        });
    }
    let delta = GridDelta {
        format: FORMAT,
        kind: Kind::Wgi,
        ops,
    };
    Ok(match checked(Delta::Wgi(delta), base, mine) {
        Some(Delta::Wgi(delta)) => Some(delta),
        _ => None,
    })
}

/// The twelve bytes of a field the game does not read; nothing is zeros.
fn unused(bytes: &Hex, name: &str) -> Result<[u8; 12]> {
    if bytes.0.is_empty() {
        return Ok([0; 12]);
    }
    bytes.0.as_slice().try_into().map_err(|_| {
        Error::InvalidArgument(format!(
            "`{name}` of a new cell is 12 bytes, not {}",
            bytes.0.len()
        ))
    })
}

/// Applies `delta` to the grid `base`. A cell to remove that is not in the
/// grid, and a new cell that needs a stream the grid does not list, are
/// skipped and listed; a new cell of a stream that has one is an error.
pub fn apply(delta: &GridDelta, base: &[u8]) -> Result<Applied> {
    check_head(delta.format, delta.kind, Kind::Wgi)?;
    let mut grid = WorldGrid::parse(base)?;
    let mut report = Report::default();
    for (i, op) in delta.ops.iter().enumerate() {
        match op {
            Op::Remove { stream } => {
                let target = op.target();
                match grid.cells.iter().position(|c| c.guid == *stream) {
                    Some(index) => {
                        grid.remove_cell(index)?;
                        grid.trim_pool();
                        report.applied();
                    }
                    None => report.skip(i, target, "the grid has no such cell"),
                }
            }
            Op::Add {
                stream,
                centre,
                radius,
                box_min,
                box_max,
                kind,
                flags,
                lod_mask,
                min_distance,
                max_distance,
                name_hash,
                linked_stream,
                needs,
                unused_24,
                reserved_64,
                unused_74,
            } => {
                let target = op.target();
                let hash = crate::toc::guid_hash(&stream.0);
                if grid.cells.iter().any(|c| c.guid == *stream) || grid.find_cell(hash).is_some() {
                    return Err(Error::InvalidArgument(format!(
                        "operation {}: the grid already has a cell of stream {stream}",
                        i + 1
                    )));
                }
                let found: std::result::Result<Vec<u32>, &Guid> = needs
                    .iter()
                    .map(|guid| {
                        grid.streams
                            .iter()
                            .position(|s| s.guid == *guid)
                            .map(|s| s as u32)
                            .ok_or(guid)
                    })
                    .collect();
                let list = match found {
                    Ok(list) => list,
                    Err(guid) => {
                        report.skip(
                            i,
                            target,
                            format!(
                                "the cell needs stream {guid}, which is not in the stream table \
                                 of the grid"
                            ),
                        );
                        continue;
                    }
                };
                let index = grid.cells.len() as u32;
                let needs = grid.intern_list(&list);
                grid.cells.push(Cell {
                    guid: *stream,
                    stream_hash: Hash32(hash),
                    centre: centre.map(|f| f.0),
                    radius: radius.0,
                    unused_24: unused(unused_24, "unused_24")?,
                    box_min: box_min.map(|f| f.0),
                    box_max: box_max.map(|f| f.0),
                    kind: *kind,
                    flags: *flags,
                    lod_mask: *lod_mask,
                    min_distance: min_distance.0,
                    max_distance: max_distance.0,
                    name_hash: *name_hash,
                    reserved_64: *reserved_64,
                    linked_stream: *linked_stream,
                    needs,
                    ring_next: index,
                    unused_74: unused(unused_74, "unused_74")?,
                });
                report.applied();
            }
        }
    }
    let data = if report.any_applied() {
        grid.to_bytes()?
    } else {
        base.to_vec()
    };
    Ok(report.finish(data))
}

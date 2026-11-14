//! "Move into the workspace" (stage 47): objects moved and turned in the
//! running game through the live link become records of their SimGroups —
//! the matrix `CMD_LoadMatrix` of each, as `level move` and the gizmo of
//! the level tab write it — in one undoable operation of the workspace.
//!
//! Hiding and showing are not written: SimGroup has no attribute for an
//! object kept out of the world (the game decides that while it runs), so
//! they stay edits of the running game only.

use crate::game::GameInstance;
use crate::level::Location;
use crate::level::edit as level_edit;
use crate::level::model::RowMatrix;
use crate::workspace::Workspace;
use crate::{Error, Result};

use super::names::LiveNames;
use super::proto::{Guid, Matrix};

/// One object to write: where its SimGroup is, the index of the object
/// there (a hint: the object is found again by GUID when the group
/// changed), and its new matrix.
#[derive(Debug, Clone, PartialEq)]
pub struct Place {
    pub guid: Guid,
    pub location: Location,
    pub object: usize,
    pub matrix: Matrix,
}

/// What [`commit`] wrote.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Committed {
    /// Objects written: GUID, SimGroup, object index, and whether the
    /// workspace had another matrix before.
    pub written: Vec<(Guid, Location, usize, bool)>,
    /// Objects not written and why.
    pub skipped: Vec<(Guid, String)>,
    /// The operation of the workspace (its label), when anything changed.
    pub operation: Option<String>,
}

/// The rows of a matrix as the level code keeps them.
pub fn rows(m: &Matrix) -> RowMatrix {
    [
        [m[0], m[1], m[2], m[3]],
        [m[4], m[5], m[6], m[7]],
        [m[8], m[9], m[10], m[11]],
        [m[12], m[13], m[14], m[15]],
    ]
}

/// Where the SimGroups of the game (the workspace's version where it has
/// one) keep the object with this GUID: its group and its index there.
pub fn locate(game: &GameInstance, names: &LiveNames, guid: &Guid) -> Option<(Location, usize)> {
    let o = names.object(guid)?;
    let stream = game
        .streams()
        .iter()
        .position(|f| f.stream_name() == o.stream)?;
    Some((
        Location {
            stream,
            resource: o.resource,
        },
        o.object,
    ))
}

/// Writes the matrices of `places` into the workspace as one operation.
pub fn commit(game: &GameInstance, ws: &mut Workspace, places: &[Place]) -> Result<Committed> {
    let mut out = Committed::default();
    let since = ws.head();
    for p in places {
        let group = match level_edit::current_group(game, ws, p.location) {
            Ok(g) => g,
            Err(e) => {
                out.skipped.push((p.guid, format!("its SimGroup: {e}")));
                continue;
            }
        };
        let index = if group
            .objects
            .get(p.object)
            .is_some_and(|o| o.guid.0 == p.guid)
        {
            Some(p.object)
        } else {
            group.objects.iter().position(|o| o.guid.0 == p.guid)
        };
        let Some(index) = index else {
            out.skipped
                .push((p.guid, "no longer in its SimGroup".to_owned()));
            continue;
        };
        if group.transform(&group.objects[index]).is_none() {
            out.skipped.push((
                p.guid,
                "its record has no matrix (CMD_LoadMatrix): the game places it otherwise"
                    .to_owned(),
            ));
            continue;
        }
        let changed = level_edit::set_transform(game, ws, p.location, index, rows(&p.matrix))?;
        out.written.push((p.guid, p.location, index, changed));
    }
    let n = ws.head().saturating_sub(since);
    if n > 1 {
        let label = format!(
            "Move {} object(s) as in the running game",
            out.written.iter().filter(|w| w.3).count()
        );
        ws.squash(since, &label)?;
    }
    if ws.head() > since {
        out.operation = ws
            .operations()
            .get(ws.head() - 1)
            .map(|op| op.label.clone());
    }
    Ok(out)
}

/// [`commit`] for the moves of the link's journal, each object found by
/// GUID through `names`.
pub fn commit_moves(
    game: &GameInstance,
    ws: &mut Workspace,
    names: &LiveNames,
    moves: &[(Guid, Matrix)],
) -> Result<Committed> {
    let mut places = Vec::new();
    let mut lost = Vec::new();
    for (guid, matrix) in moves {
        match locate(game, names, guid) {
            Some((location, object)) => places.push(Place {
                guid: *guid,
                location,
                object,
                matrix: *matrix,
            }),
            None => lost.push((
                *guid,
                "no SimGroup of the game or the workspace has it (made by the game while it runs?)"
                    .to_owned(),
            )),
        }
    }
    if places.is_empty() && !moves.is_empty() && lost.len() == moves.len() {
        return Err(Error::InvalidArgument(format!(
            "none of the {} moved object(s) is in a SimGroup",
            moves.len()
        )));
    }
    let mut out = commit(game, ws, &places)?;
    out.skipped.extend(lost);
    Ok(out)
}

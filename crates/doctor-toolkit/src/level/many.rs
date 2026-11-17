//! Several objects in one undoable operation: what a multiple selection of
//! the level view does (move, duplicate, delete), for every surface.
//!
//! Each function records the single edits of [`super::edit`] and joins
//! them ([`Workspace::squash`]), so that one undo takes the whole action
//! back.

use crate::game::GameInstance;
use crate::resource::simgroup::Guid;
use crate::workspace::Workspace;
use crate::{Error, Result};

use super::RowMatrix;
use super::edit::{self, GroupAt};

/// An object: its SimGroup and its number there.
pub type ObjectAt = (GroupAt, usize);

fn check_distinct(objects: &[ObjectAt]) -> Result<()> {
    for (i, a) in objects.iter().enumerate() {
        if objects[..i].contains(a) {
            return Err(Error::InvalidArgument(format!(
                "object {} is named twice",
                a.1
            )));
        }
    }
    Ok(())
}

/// Joins what was recorded since `since` under `label`; a single edit keeps
/// its own label.
fn join(ws: &mut Workspace, since: usize, label: &str) -> Result<()> {
    if ws.head() > since + 1 {
        ws.squash(since, label)?;
    }
    Ok(())
}

/// Gives each object its new world matrix: one undoable operation. Returns
/// how many objects changed.
pub fn transform_many(
    game: &GameInstance,
    ws: &mut Workspace,
    objects: &[(GroupAt, usize, RowMatrix)],
) -> Result<usize> {
    let since = ws.head();
    let mut changed = 0;
    let done = (|| -> Result<()> {
        for (at, object, matrix) in objects {
            changed += edit::set_transform_at(game, ws, at, *object, *matrix)? as usize;
        }
        Ok(())
    })();
    if let Err(e) = done {
        ws.rollback(since)?;
        return Err(e);
    }
    join(ws, since, &format!("Move {changed} objects"))?;
    Ok(changed)
}

/// Copies each object, the copies moved by `offset`: one undoable
/// operation. Returns the copies with their GUIDs, in the order of `objects`.
pub fn duplicate_many(
    game: &GameInstance,
    ws: &mut Workspace,
    objects: &[ObjectAt],
    offset: [f32; 3],
    taken: &dyn Fn(&Guid) -> bool,
) -> Result<Vec<(GroupAt, usize, Guid)>> {
    check_distinct(objects)?;
    let since = ws.head();
    let mut copies = Vec::new();
    let done = (|| -> Result<()> {
        for (at, object) in objects {
            let (copy, guid) = edit::duplicate_at(game, ws, at, *object, offset, taken)?;
            copies.push((at.clone(), copy, guid));
        }
        Ok(())
    })();
    if let Err(e) = done {
        ws.rollback(since)?;
        return Err(e);
    }
    join(ws, since, &format!("Duplicate {} objects", objects.len()))?;
    Ok(copies)
}

/// Removes the objects: one undoable operation. The objects of a SimGroup
/// go from the last to the first, so that the numbers given stay true
/// while they go. Returns how many attributes of other objects still name
/// the GUID of a removed one.
pub fn remove_many(game: &GameInstance, ws: &mut Workspace, objects: &[ObjectAt]) -> Result<usize> {
    check_distinct(objects)?;
    let mut order: Vec<&ObjectAt> = objects.iter().collect();
    order.sort_by_key(|o| std::cmp::Reverse(o.1));
    let since = ws.head();
    let mut dangling = 0;
    let done = (|| -> Result<()> {
        for (at, object) in order {
            dangling += edit::remove_at(game, ws, at, *object)?;
        }
        Ok(())
    })();
    if let Err(e) = done {
        ws.rollback(since)?;
        return Err(e);
    }
    join(ws, since, &format!("Remove {} objects", objects.len()))?;
    Ok(dangling)
}

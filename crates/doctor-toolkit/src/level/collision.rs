//! The Havok collision of a level placed in the world: every
//! `CollisionHavok4Part` of the expanded model of every placed object, with
//! the matrix that puts it into world space (stage 37, walking a level).
//!
//! Collision shapes lie in the model space of their metamodel, like its
//! meshes (`docs/formats/havok.md` §1.3), so the world matrix is the part's
//! placement composed with the object's transform. Terrain objects stand at
//! the origin: their collision is in world coordinates.

use std::sync::Arc;

use super::{Level, LevelSource, RowMatrix, compose};
use crate::resource::hkx::TYPE_HKX;
use crate::resource::hkx::physics::Physics;
use crate::{Error, Result};

/// A collision packfile placed in the world by an object of the level.
#[derive(Debug, Clone, PartialEq)]
pub struct PlacedCollision {
    /// Index of the SimGroup in [`Level::groups`].
    pub group: usize,
    /// Index of the object in its SimGroup.
    pub object: usize,
    /// Resource id of the `hkx`.
    pub hkx: u32,
    /// Model space of the collision to world space (row vectors).
    pub matrix: RowMatrix,
}

impl Level {
    /// Every collision of every placed object, in the order of the groups
    /// and objects.
    pub fn collision_placements(&self) -> Vec<PlacedCollision> {
        let mut out = Vec::new();
        for (g, group) in self.groups.iter().enumerate() {
            for (o, object) in group.objects.iter().enumerate() {
                let (Some(transform), Some(model)) = (object.transform, object.model) else {
                    continue;
                };
                let Some(model) = self.models.get(&model) else {
                    continue;
                };
                for marker in &model.desc.collisions {
                    out.push(PlacedCollision {
                        group: g,
                        object: o,
                        hkx: marker.resource,
                        matrix: compose(&marker.matrix, &transform),
                    });
                }
            }
        }
        out
    }
}

impl LevelSource {
    /// The collision packfile with id `id`, parsed (the workspace version
    /// when it was edited or added).
    pub fn physics(&self, id: u32) -> Result<Arc<Physics>> {
        let (_, data) = self
            .data(TYPE_HKX, id)
            .ok_or_else(|| Error::InvalidArgument(format!("hkx {id:08x} not found")))?;
        Ok(Arc::new(Physics::parse(&data)?))
    }

    /// Whether the collision `id` comes from the workspace (edited or new).
    pub fn collision_overridden(&self, id: u32) -> bool {
        self.overrides.by_id.contains_key(&(TYPE_HKX, id))
    }
}

//! What a placed object looks like: a `metamodel` expanded into meshes with
//! their matrices relative to the object.
//!
//! The expansion follows `docs/levels.md` §2:
//!
//! - states: `Transition` states are skipped; of `LodState` states every one
//!   is kept with its distance range; of every other state type only the
//!   first (the initial state: `DestructibleState_0`, the first `KnobState`);
//! - parts of the kept states: `ModelPart.Model` (also `DynamicModelPart`,
//!   `LODAwareModelPart`, `UpgradeModelPart`) is a mesh at the object
//!   origin; `Entity.CMD_SetModel` (parts `Destructible`, `Door`,
//!   `BuildingWindow`) and `MetaModelPart.fileName` are nested metamodels
//!   placed by `CSystemCommands.CMD_LoadMatrix`, `MetaModelPart.CMD_LoadMatrix`
//!   or `TransformedPart.Transform`; `TreeModelPart.TreeModel` is a tree (type
//!   `0xCDB9D42B`, not decoded) at `TreeModelPart.CMD_LoadMatrix`;
//!   `MinimapPart.Model` (the map icon) is skipped;
//!   `CollisionHavok4Part.CollisionModel` is the collision (`hkx`) of the
//!   object, in the same space as its meshes.
//!
//! Every reference is resolved by its GUID
//! ([`metamodel::ResourceRef::target_id`]), as the game and `check-refs`
//! do, not by the path hash: for nested metamodels split by level of detail
//! the path hash names the unsplit original or nothing.

use crate::resource::hkx::{TYPE_HKX, TYPE_HKX_REFERENCE};

use crate::resource::metamodel::{self, MetaModel, PropValue};

/// Type id of `mesh`.
pub const TYPE_MESH: u32 = crate::hash::sdbm(b"mesh");
/// Type id of `metamodel`.
pub const TYPE_METAMODEL: u32 = crate::hash::sdbm(b"metamodel");
/// Type id of tree models, `treemodel` (`0xCDB9D42B`, `TreeModelPart.TreeModel`).
pub const TYPE_TREE: u32 = crate::hash::sdbm(b"treemodel");

/// Nested metamodels deeper than this are not expanded (the game has 3).
pub const MAX_DEPTH: usize = 8;

/// Row-major 4×4 matrix with rows right, up, at, position (row vectors).
pub type RowMatrix = [[f32; 4]; 4];

pub const IDENTITY: RowMatrix = [
    [1.0, 0.0, 0.0, 0.0],
    [0.0, 1.0, 0.0, 0.0],
    [0.0, 0.0, 1.0, 0.0],
    [0.0, 0.0, 0.0, 1.0],
];

/// `local` placed in `parent`: the matrix that maps local points into the
/// parent's parent space (`p · local · parent` with row vectors).
pub fn compose(local: &RowMatrix, parent: &RowMatrix) -> RowMatrix {
    let mut out = [[0f32; 4]; 4];
    for (i, row) in out.iter_mut().enumerate() {
        for (j, value) in row.iter_mut().enumerate() {
            *value = (0..4).map(|k| local[i][k] * parent[k][j]).sum();
        }
    }
    out
}

/// A distance range `[near, far)` in which a level of detail is shown.
pub type LodRange = [f32; 2];

/// One mesh of an expanded model.
#[derive(Debug, Clone, PartialEq)]
pub struct Piece {
    /// Resource id of the mesh.
    pub mesh: u32,
    /// Placement relative to the object.
    pub matrix: RowMatrix,
    /// Shown only at camera distances in this range; `None` always.
    pub lod: Option<LodRange>,
}

/// Something of a model that is not a mesh but has a place: a tree.
#[derive(Debug, Clone, PartialEq)]
pub struct Marker {
    pub resource: u32,
    pub matrix: RowMatrix,
}

/// A metamodel expanded into meshes.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ModelDesc {
    pub pieces: Vec<Piece>,
    pub trees: Vec<Marker>,
    /// Collision packfiles (`hkx` ids) with their placement.
    pub collisions: Vec<Marker>,
    /// Metamodels (nested ones included) that were not found.
    pub missing: Vec<u32>,
    /// Number of metamodels expanded, the model itself included.
    pub metamodels: usize,
}

/// Finds and parses metamodels by resource id.
pub trait MetaModels {
    fn metamodel(&mut self, id: u32) -> Option<std::sync::Arc<MetaModel>>;
}

/// Expands metamodel `id` and everything nested in it.
pub fn expand(id: u32, source: &mut dyn MetaModels) -> ModelDesc {
    let mut out = ModelDesc::default();
    let mut stack = Vec::new();
    expand_into(id, &IDENTITY, None, source, &mut stack, &mut out);
    out
}

fn expand_into(
    id: u32,
    parent: &RowMatrix,
    lod: Option<LodRange>,
    source: &mut dyn MetaModels,
    stack: &mut Vec<u32>,
    out: &mut ModelDesc,
) {
    if stack.len() >= MAX_DEPTH || stack.contains(&id) {
        return;
    }
    let Some(mm) = source.metamodel(id) else {
        out.missing.push(id);
        return;
    };
    out.metamodels += 1;
    stack.push(id);
    for (state, state_lod) in selected_states(&mm) {
        let lod = intersect(lod, state_lod);
        for &part_index in &mm.states[state].parts {
            let Some(part) = mm.parts.get(part_index) else {
                continue;
            };
            if part.type_name == "MinimapPart" {
                continue;
            }
            let mut matrix = None;
            let mut mesh = None;
            let mut nested = None;
            let mut tree = None;
            let mut collision = None;
            for &p in &part.properties {
                let Some(prop) = mm.properties.get(p) else {
                    continue;
                };
                let resource = |value: &PropValue| match value {
                    PropValue::Resource(r) => mm.resources.get(*r),
                    _ => None,
                };
                match (prop.class.as_str(), prop.name.as_str(), &prop.value) {
                    (_, "CMD_LoadMatrix" | "Transform", PropValue::Matrix(m)) => {
                        // CMD_LoadMatrix wins over TransformedPart.Transform.
                        if matrix.is_none() || prop.name == "CMD_LoadMatrix" {
                            matrix = Some(*m);
                        }
                    }
                    ("ModelPart", "Model", v) => {
                        mesh = resource(v).filter(|r| r.type_id.0 == TYPE_MESH);
                    }
                    ("Entity", "CMD_SetModel", v) | ("MetaModelPart", "fileName", v) => {
                        nested = resource(v).filter(|r| r.type_id.0 == TYPE_METAMODEL);
                    }
                    ("TreeModelPart", "TreeModel", v) => {
                        tree = resource(v).filter(|r| r.type_id.0 == TYPE_TREE);
                    }
                    ("CollisionHavok4Part", "CollisionModel", v) => {
                        collision = resource(v).filter(|r| {
                            r.type_id.0 == TYPE_HKX_REFERENCE || r.type_id.0 == TYPE_HKX
                        });
                    }
                    _ => {}
                }
            }
            let local = matrix.unwrap_or(IDENTITY);
            let placed = compose(&local, parent);
            if let Some(r) = mesh {
                // Model parts have no matrix of their own in the game.
                out.pieces.push(Piece {
                    mesh: r.target_id(),
                    matrix: placed,
                    lod,
                });
            }
            if let Some(r) = nested {
                expand_into(r.target_id(), &placed, lod, source, stack, out);
            }
            if let Some(r) = tree {
                out.trees.push(Marker {
                    resource: r.target_id(),
                    matrix: placed,
                });
            }
            if let Some(r) = collision {
                out.collisions.push(Marker {
                    resource: r.target_id(),
                    matrix: placed,
                });
            }
        }
    }
    stack.pop();
}

fn intersect(a: Option<LodRange>, b: Option<LodRange>) -> Option<LodRange> {
    match (a, b) {
        (Some(a), Some(b)) => Some([a[0].max(b[0]), a[1].min(b[1])]),
        (a, b) => a.or(b),
    }
}

/// The states drawn by default, with the distance range of LOD states.
pub fn selected_states(mm: &MetaModel) -> Vec<(usize, Option<LodRange>)> {
    let mut seen_types: Vec<&str> = Vec::new();
    let mut out = Vec::new();
    for (i, state) in mm.states.iter().enumerate() {
        match state.type_name.as_str() {
            "Transition" => {}
            "LodState" => out.push((i, Some(lod_range(mm, i)))),
            other => {
                if !seen_types.contains(&other) {
                    seen_types.push(other);
                    out.push((i, None));
                }
            }
        }
    }
    out
}

/// The distance range of a `LodState` from its conditions on the float
/// variable `distance`: operator ≥ ([`metamodel::OP_GE`]) gives the near
/// end, ≤ ([`metamodel::OP_LE`]) the far end (all 427 LOD states of the game
/// have one of each).
pub fn lod_range(mm: &MetaModel, state: usize) -> LodRange {
    let mut range = [0.0, f32::INFINITY];
    for &c in &mm.states[state].conditions {
        let Some(condition) = mm.conditions.get(c) else {
            continue;
        };
        let Some(variable) = mm.variables.get(condition.variable) else {
            continue;
        };
        if variable.name != "distance" || variable.kind != 4 {
            continue;
        }
        let value = f32::from_bits(condition.value);
        match condition.operator {
            metamodel::OP_GE => range[0] = value,
            metamodel::OP_LE => range[1] = value,
            _ => {}
        }
    }
    range
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compose_applies_the_local_matrix_first() {
        let mut parent = IDENTITY;
        parent[3] = [10.0, 0.0, 0.0, 1.0];
        // 90° about Y in row form: right becomes -Z.
        let mut local = [
            [0.0, 0.0, -1.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [1.0, 0.0, 0.0, 0.0],
            [0.0; 4],
        ];
        local[3] = [0.0, 0.0, 2.0, 1.0];
        let m = compose(&local, &parent);
        assert_eq!(m[3], [10.0, 0.0, 2.0, 1.0]);
        assert_eq!(m[0], [0.0, 0.0, -1.0, 0.0]);
        // A rotated parent turns the local offset.
        let m = compose(&parent, &local);
        assert_eq!(m[3], [0.0, 0.0, -8.0, 1.0]);
    }

    #[test]
    fn lod_ranges_intersect() {
        assert_eq!(intersect(None, Some([1.0, 2.0])), Some([1.0, 2.0]));
        assert_eq!(
            intersect(Some([0.0, 10.0]), Some([5.0, 20.0])),
            Some([5.0, 10.0])
        );
        assert_eq!(intersect(None, None), None);
    }
}

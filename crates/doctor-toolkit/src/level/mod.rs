//! Levels: the placed objects of a set of streams with the models they show.
//!
//! A level is a set of streams chosen by the user (a zone such as
//! `gf2/miami/local/bonita/high/terrain06`, or every zone of a district).
//! [`Level::load`] reads their SimGroups and point tables and expands the
//! `metamodel` of every object into meshes ([`model`]). References are
//! resolved like the game does: in the chosen streams first, then in their
//! ancestors by the tables of contents (`globals_global` → `miami_global` →
//! `miami` → `miami\local` → zones), then anywhere in the game (counted as
//! out of scope in the [`Coverage`]).
//!
//! The data can be overridden with edited or added resources of a workspace
//! ([`Overrides`]) so that the level shows the edits before they are applied.
//! See `docs/levels.md`.

pub mod assets;
pub mod catalog;
pub mod collision;
pub mod compat;
pub mod edit;
pub mod model;
pub mod navgraph;

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use rayon::prelude::*;

use crate::game::{GameInstance, ResourceIndex, StreamFile};
use crate::resource::hkx::TYPE_HKX;
use crate::resource::metamodel::MetaModel;
use crate::resource::points::{Point, PointTable, TYPE_POINTS};
use crate::resource::simgroup::{CLASS_SYSTEM_COMMANDS, Guid, SimGroup};
use crate::{Error, Result};

pub use assets::LevelAssets;
pub use catalog::{Catalog, Zone};
pub use collision::PlacedCollision;
pub use model::{
    IDENTITY, Light, LightKind, LodRange, Marker, ModelDesc, Piece, RowMatrix, TYPE_MESH,
    TYPE_METAMODEL, TYPE_TREE, compose,
};

/// Type id of `simgroup`.
pub const TYPE_SIMGROUP: u32 = crate::hash::sdbm(b"simgroup");
/// Type id of `meshvolatile`.
pub const TYPE_MESH_VOLATILE: u32 = crate::hash::sdbm(b"meshvolatile");

/// Where a resource lives in the game files.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Location {
    pub stream: usize,
    pub resource: usize,
}

/// Resources of the game by type and last id, built once from the index.
#[derive(Debug, Default)]
pub struct Lookup {
    by_id: HashMap<(u32, u32), Vec<Location>>,
    /// Resources of each stream that a level reads: SimGroups, point tables,
    /// materials.
    by_stream: HashMap<usize, Vec<(u32, usize)>>,
}

impl Lookup {
    pub fn new(index: &ResourceIndex) -> Self {
        let mut lookup = Self::default();
        for e in &index.entries {
            let t = e.header.type_id;
            let location = Location {
                stream: e.stream as usize,
                resource: e.resource as usize,
            };
            if [TYPE_MESH, TYPE_METAMODEL, TYPE_TREE, TYPE_HKX].contains(&t) {
                lookup
                    .by_id
                    .entry((t, e.header.ids[3]))
                    .or_default()
                    .push(location);
            }
            if [TYPE_SIMGROUP, TYPE_POINTS, TYPE_MATERIAL].contains(&t) {
                lookup
                    .by_stream
                    .entry(location.stream)
                    .or_default()
                    .push((t, location.resource));
            }
        }
        lookup
    }

    /// Every location of a resource.
    pub fn locations(&self, type_id: u32, id: u32) -> &[Location] {
        self.by_id.get(&(type_id, id)).map_or(&[], Vec::as_slice)
    }

    /// Resources of `type_id` in stream `stream`.
    pub fn in_stream(&self, stream: usize, type_id: u32) -> impl Iterator<Item = usize> + '_ {
        self.by_stream
            .get(&stream)
            .into_iter()
            .flatten()
            .filter(move |(t, _)| *t == type_id)
            .map(|&(_, r)| r)
    }
}

const TYPE_MATERIAL: u32 = crate::hash::sdbm(b"material");

/// Data of a resource with the stream it is in.
pub type StreamData = (usize, Arc<Vec<u8>>);

/// Data of edited or added resources that replaces the game files.
#[derive(Debug, Clone, Default)]
pub struct Overrides {
    /// Resources referenced by id (`metamodel`, `mesh`, `meshvolatile`),
    /// keyed by type and last id: the stream they are in and their data.
    pub by_id: HashMap<(u32, u32), StreamData>,
    /// SimGroups (their ids are zero): keyed by location.
    pub groups: HashMap<Location, Arc<Vec<u8>>>,
}

/// The streams a level reads from, nearest first.
#[derive(Debug, Clone, Default)]
pub struct Scope {
    /// Chosen streams, then their ancestors.
    pub streams: Vec<usize>,
    rank: HashMap<usize, usize>,
}

impl Scope {
    /// The chosen streams and their ancestors in the tables of contents.
    pub fn new(
        game: &GameInstance,
        parents: &HashMap<String, Vec<String>>,
        chosen: &[usize],
    ) -> Self {
        let mut streams: Vec<usize> = Vec::new();
        let push = |s: usize, streams: &mut Vec<usize>| {
            if !streams.contains(&s) {
                streams.push(s);
            }
        };
        for &s in chosen {
            push(s, &mut streams);
        }
        for &s in chosen {
            let name = game.streams()[s].stream_name();
            for ancestor in crate::game::ancestors(parents, &name) {
                if let Some(a) = game.find_stream(&ancestor) {
                    push(a, &mut streams);
                }
            }
        }
        let rank = streams.iter().enumerate().map(|(i, &s)| (s, i)).collect();
        Self { streams, rank }
    }

    pub fn contains(&self, stream: usize) -> bool {
        self.rank.contains_key(&stream)
    }

    /// Position of a stream in [`Self::streams`]; `usize::MAX` outside.
    pub fn rank(&self, stream: usize) -> usize {
        self.rank.get(&stream).copied().unwrap_or(usize::MAX)
    }

    /// The location nearest in scope; else the first one anywhere, with `false`.
    pub fn pick(&self, locations: &[Location]) -> Option<(Location, bool)> {
        locations
            .iter()
            .filter_map(|l| self.rank.get(&l.stream).map(|r| (*r, *l)))
            .min_by_key(|(r, _)| *r)
            .map(|(_, l)| (l, true))
            .or_else(|| locations.first().map(|l| (*l, false)))
    }
}

/// Reads resources for a level: game files with overrides, streams opened once.
pub struct LevelSource {
    pub game: Arc<GameInstance>,
    pub lookup: Arc<Lookup>,
    pub overrides: Overrides,
    pub scope: Scope,
    open: Mutex<HashMap<usize, Arc<StreamFile>>>,
    metamodels: Mutex<HashMap<u32, Option<Arc<MetaModel>>>>,
}

/// Where a referenced resource was found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Found {
    /// In a game stream; `true` when the stream is in the scope.
    Game(Location, bool),
    /// An edited or added resource of the workspace, in this stream.
    Override(usize, bool),
}

impl Found {
    pub fn stream(&self) -> usize {
        match *self {
            Found::Game(l, _) => l.stream,
            Found::Override(s, _) => s,
        }
    }

    pub fn in_scope(&self) -> bool {
        match *self {
            Found::Game(_, s) | Found::Override(_, s) => s,
        }
    }
}

impl LevelSource {
    pub fn new(
        game: Arc<GameInstance>,
        lookup: Arc<Lookup>,
        overrides: Overrides,
        scope: Scope,
    ) -> Self {
        Self {
            game,
            lookup,
            overrides,
            scope,
            open: Mutex::default(),
            metamodels: Mutex::default(),
        }
    }

    /// The stream file, opened once.
    pub fn stream(&self, index: usize) -> Result<Arc<StreamFile>> {
        if let Some(s) = self
            .open
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&index)
        {
            return Ok(s.clone());
        }
        let stream = Arc::new(self.game.open_stream(index)?);
        self.open
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(index, stream.clone());
        Ok(stream)
    }

    /// Closes the streams opened so far (before their files are replaced).
    pub fn close_streams(&self) {
        self.open.lock().unwrap_or_else(|e| e.into_inner()).clear();
    }

    /// Finds a resource by type and id: overrides in scope, game streams in
    /// scope, overrides anywhere, game streams anywhere.
    pub fn find(&self, type_id: u32, id: u32) -> Option<Found> {
        let over = self
            .overrides
            .by_id
            .get(&(type_id, id))
            .map(|(s, _)| Found::Override(*s, self.scope.contains(*s)));
        if let Some(found) = over
            && found.in_scope()
        {
            return Some(found);
        }
        let game = self
            .scope
            .pick(self.lookup.locations(type_id, id))
            .map(|(l, s)| Found::Game(l, s));
        match game {
            Some(found) if found.in_scope() => Some(found),
            other => over.or(other),
        }
    }

    /// Data of a resource by type and id (see [`Self::find`]).
    pub fn data(&self, type_id: u32, id: u32) -> Option<(Found, Vec<u8>)> {
        let found = self.find(type_id, id)?;
        let data = match found {
            Found::Override(..) => self.overrides.by_id[&(type_id, id)].1.to_vec(),
            Found::Game(l, _) => self.stream(l.stream).ok()?.resource_data(l.resource).ok()?,
        };
        Some((found, data))
    }

    /// Data of a resource of a stream, with the SimGroup overrides.
    pub fn resource_data(&self, location: Location) -> Result<Vec<u8>> {
        if let Some(data) = self.overrides.groups.get(&location) {
            return Ok(data.to_vec());
        }
        self.stream(location.stream)?
            .resource_data(location.resource)
    }

    /// A parsed metamodel by id, cached.
    pub fn metamodel(&self, id: u32) -> Option<Arc<MetaModel>> {
        if let Some(m) = self
            .metamodels
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&id)
        {
            return m.clone();
        }
        let parsed = self
            .data(TYPE_METAMODEL, id)
            .and_then(|(_, data)| MetaModel::parse(&data).ok())
            .map(Arc::new);
        self.metamodels
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id, parsed.clone());
        parsed
    }

    /// The mesh with id `id` as a model: its `mesh` and `meshvolatile` data.
    pub fn mesh(&self, id: u32) -> Result<crate::resource::mesh::Model> {
        let (found, mesh) = self
            .data(TYPE_MESH, id)
            .ok_or_else(|| Error::InvalidArgument(format!("mesh {id:08x} not found")))?;
        let volatile = match found {
            Found::Override(..) => self
                .overrides
                .by_id
                .get(&(TYPE_MESH_VOLATILE, !id))
                .map(|(_, d)| d.to_vec()),
            Found::Game(l, _) => {
                let stream = self.stream(l.stream)?;
                crate::resource::mesh::volatile_for(stream.layout(), l.resource)
                    .map(|v| stream.resource_data(v))
                    .transpose()?
            }
        };
        crate::resource::mesh::Model::parse(&mesh, &volatile.unwrap_or_default())
    }

    /// The tree model with id `id`.
    pub fn tree(&self, id: u32) -> Result<crate::resource::treemodel::TreeModel> {
        let (_, data) = self
            .data(TYPE_TREE, id)
            .ok_or_else(|| Error::InvalidArgument(format!("treemodel {id:08x} not found")))?;
        crate::resource::treemodel::TreeModel::parse(&data)
    }
}

impl LevelSource {
    /// The collision packfile with id `id` as triangles in its model space.
    pub fn collision(&self, id: u32) -> Result<crate::resource::hkx::geometry::TriangleMesh> {
        let (_, data) = self
            .data(TYPE_HKX, id)
            .ok_or_else(|| Error::InvalidArgument(format!("hkx {id:08x} not found")))?;
        let physics = crate::resource::hkx::physics::Physics::parse(&data)?;
        Ok(crate::resource::hkx::geometry::physics_mesh(&physics))
    }
}

impl model::MetaModels for &LevelSource {
    fn metamodel(&mut self, id: u32) -> Option<Arc<MetaModel>> {
        LevelSource::metamodel(self, id)
    }
}

/// One object of a level as the view needs it.
#[derive(Debug, Clone, PartialEq)]
pub struct LevelObject {
    /// Index of the object in its SimGroup.
    pub index: usize,
    pub class: u32,
    pub class_name: Option<&'static str>,
    pub name: String,
    pub guid: Guid,
    /// World transform; identity for objects with a `CSystemCommands`
    /// packet but no matrix (terrain is placed so); `None` for objects
    /// without a position (logic objects).
    pub transform: Option<RowMatrix>,
    /// Whether the matrix is stored (not the implicit identity).
    pub has_matrix: bool,
    /// Id of the `metamodel` the object shows.
    pub model: Option<u32>,
}

/// A SimGroup of the level with its objects.
#[derive(Debug, Clone)]
pub struct LevelGroup {
    pub location: Location,
    pub group: SimGroup,
    pub objects: Vec<LevelObject>,
}

impl LevelGroup {
    pub fn new(location: Location, group: SimGroup) -> Self {
        let objects = group
            .objects
            .iter()
            .enumerate()
            .map(|(index, o)| {
                let matrix = group.transform(o);
                let placed = o
                    .packets
                    .iter()
                    .any(|&p| group.packets[p].class.0 == CLASS_SYSTEM_COMMANDS);
                LevelObject {
                    index,
                    class: o.class,
                    class_name: o.class_name(),
                    name: group.object_name(o).unwrap_or_default().to_owned(),
                    guid: o.guid,
                    transform: matrix.or(placed.then_some(IDENTITY)),
                    has_matrix: matrix.is_some(),
                    model: group.model(o),
                }
            })
            .collect();
        Self {
            location,
            group,
            objects,
        }
    }
}

/// A point table of the level.
#[derive(Debug, Clone)]
pub struct LevelPoints {
    pub location: Location,
    pub points: Vec<Point>,
}

/// How a model was resolved.
#[derive(Debug, Clone, PartialEq)]
pub struct LevelModel {
    pub desc: ModelDesc,
    /// Where the metamodel was found; `None` when it is not in the game.
    pub found: Option<Found>,
}

/// A mesh used by the level.
#[derive(Debug, Clone, PartialEq)]
pub struct LevelMesh {
    pub id: u32,
    pub found: Option<Found>,
}

/// How much of a level can be shown.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
pub struct Coverage {
    pub streams: usize,
    pub scope_streams: usize,
    pub simgroups: usize,
    pub objects: usize,
    /// Objects with a position (a `CSystemCommands` packet).
    pub placed: usize,
    /// Objects with a model (`Entity.CMD_SetModel`).
    pub with_model: usize,
    /// Objects whose model gives at least one mesh that was found.
    pub rendered: usize,
    /// Objects with a model and a position that is drawn.
    pub rendered_placed: usize,
    /// Objects whose metamodel is not in the game.
    pub model_missing: usize,
    /// Objects whose metamodel was found only outside the scope.
    pub model_out_of_scope: usize,
    /// Objects whose model has no mesh (effects, lights, trees only).
    pub model_without_mesh: usize,
    /// Distinct metamodels and meshes.
    pub models: usize,
    pub meshes: usize,
    pub meshes_missing: usize,
    pub meshes_out_of_scope: usize,
    /// Mesh instances (pieces of placed objects).
    pub instances: usize,
    pub trees: usize,
    pub points: usize,
}

/// A loaded level.
pub struct Level {
    pub source: Arc<LevelSource>,
    /// The chosen streams.
    pub streams: Vec<usize>,
    pub groups: Vec<LevelGroup>,
    pub points: Vec<LevelPoints>,
    pub models: HashMap<u32, LevelModel>,
    pub meshes: Vec<LevelMesh>,
    mesh_index: HashMap<u32, usize>,
    /// Tree models (`treemodel`) of the models, found like the meshes.
    pub trees: Vec<LevelMesh>,
    tree_index: HashMap<u32, usize>,
}

impl Level {
    /// Reads the SimGroups and point tables of `streams` and expands the
    /// models of their objects.
    pub fn load(source: Arc<LevelSource>, streams: &[usize]) -> Result<Self> {
        let locations: Vec<(u32, Location)> = streams
            .iter()
            .flat_map(|&s| {
                let groups = source.lookup.in_stream(s, TYPE_SIMGROUP).map(move |r| {
                    (
                        TYPE_SIMGROUP,
                        Location {
                            stream: s,
                            resource: r,
                        },
                    )
                });
                let points = source.lookup.in_stream(s, TYPE_POINTS).map(move |r| {
                    (
                        TYPE_POINTS,
                        Location {
                            stream: s,
                            resource: r,
                        },
                    )
                });
                groups.chain(points).collect::<Vec<_>>()
            })
            .collect();
        enum Read {
            Group(LevelGroup),
            Points(LevelPoints),
        }
        let read: Vec<Read> = locations
            .par_iter()
            .map(|&(t, location)| {
                let data = source.resource_data(location)?;
                Ok(if t == TYPE_SIMGROUP {
                    Read::Group(LevelGroup::new(location, SimGroup::parse(&data)?))
                } else {
                    Read::Points(LevelPoints {
                        location,
                        points: PointTable::parse(&data)?.points,
                    })
                })
            })
            .collect::<Result<_>>()?;
        let mut level = Self {
            source,
            streams: streams.to_vec(),
            groups: Vec::new(),
            points: Vec::new(),
            models: HashMap::new(),
            meshes: Vec::new(),
            mesh_index: HashMap::new(),
            trees: Vec::new(),
            tree_index: HashMap::new(),
        };
        for r in read {
            match r {
                Read::Group(g) => level.groups.push(g),
                Read::Points(p) => level.points.push(p),
            }
        }
        level.resolve_models();
        Ok(level)
    }

    /// Expands the models of all objects that are not expanded yet.
    pub fn resolve_models(&mut self) {
        let wanted: HashSet<u32> = self
            .groups
            .iter()
            .flat_map(|g| g.objects.iter().filter_map(|o| o.model))
            .filter(|id| !self.models.contains_key(id))
            .collect();
        let wanted: Vec<u32> = wanted.into_iter().collect();
        let source = &*self.source;
        let expanded: Vec<(u32, LevelModel)> = wanted
            .par_iter()
            .map(|&id| {
                let found = source.find(TYPE_METAMODEL, id);
                let mut metamodels = source;
                let desc = model::expand(id, &mut metamodels);
                (id, LevelModel { desc, found })
            })
            .collect();
        for (id, m) in expanded {
            for piece in &m.desc.pieces {
                if !self.mesh_index.contains_key(&piece.mesh) {
                    self.mesh_index.insert(piece.mesh, self.meshes.len());
                    self.meshes.push(LevelMesh {
                        id: piece.mesh,
                        found: source.find(TYPE_MESH, piece.mesh),
                    });
                }
            }
            for tree in &m.desc.trees {
                if !self.tree_index.contains_key(&tree.resource) {
                    self.tree_index.insert(tree.resource, self.trees.len());
                    self.trees.push(LevelMesh {
                        id: tree.resource,
                        found: source.find(TYPE_TREE, tree.resource),
                    });
                }
            }
            self.models.insert(id, m);
        }
    }

    /// Index into [`Self::trees`] of a tree model id.
    pub fn tree_slot(&self, id: u32) -> Option<usize> {
        self.tree_index.get(&id).copied()
    }

    /// Index into [`Self::meshes`] of a mesh id.
    pub fn mesh_slot(&self, id: u32) -> Option<usize> {
        self.mesh_index.get(&id).copied()
    }

    /// Replaces the data of group `group` (after an edit) and expands new models.
    pub fn set_group(&mut self, group: usize, data: &[u8]) -> Result<()> {
        let g = self
            .groups
            .get_mut(group)
            .ok_or_else(|| Error::InvalidArgument(format!("no group {group}")))?;
        *g = LevelGroup::new(g.location, SimGroup::parse(data)?);
        self.resolve_models();
        Ok(())
    }

    /// Every object GUID of the level, for new GUIDs.
    pub fn guids(&self) -> HashSet<Guid> {
        self.groups
            .iter()
            .flat_map(|g| g.objects.iter().map(|o| o.guid))
            .collect()
    }

    /// Whether a mesh can be drawn: found in the game or the workspace.
    pub fn mesh_found(&self, id: u32) -> bool {
        self.mesh_slot(id)
            .is_some_and(|i| self.meshes[i].found.is_some())
    }

    pub fn coverage(&self) -> Coverage {
        let mut c = Coverage {
            streams: self.streams.len(),
            scope_streams: self.source.scope.streams.len(),
            simgroups: self.groups.len(),
            models: self.models.len(),
            meshes: self.meshes.len(),
            meshes_missing: self.meshes.iter().filter(|m| m.found.is_none()).count(),
            meshes_out_of_scope: self
                .meshes
                .iter()
                .filter(|m| m.found.is_some_and(|f| !f.in_scope()))
                .count(),
            points: self.points.iter().map(|p| p.points.len()).sum(),
            ..Coverage::default()
        };
        for o in self.groups.iter().flat_map(|g| &g.objects) {
            c.objects += 1;
            c.placed += o.transform.is_some() as usize;
            let Some(id) = o.model else { continue };
            c.with_model += 1;
            let m = &self.models[&id];
            match m.found {
                None => c.model_missing += 1,
                Some(f) if !f.in_scope() => c.model_out_of_scope += 1,
                _ => {}
            }
            let drawn = m
                .desc
                .pieces
                .iter()
                .filter(|p| self.mesh_found(p.mesh))
                .count();
            if m.found.is_some() && drawn == 0 {
                c.model_without_mesh += 1;
            }
            if o.transform.is_some() {
                c.trees += m.desc.trees.len();
            }
            if drawn > 0 {
                c.rendered += 1;
                if o.transform.is_some() {
                    c.rendered_placed += 1;
                    c.instances += drawn;
                }
            }
        }
        c
    }
}

#[cfg(test)]
mod tests;
